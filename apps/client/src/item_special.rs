//! Icons of special models (`docs/specs/items.md`, `minecraft:special` types): heads and
//! skulls, patterned banners, shields, chests, Shulker boxes. The geometry comes from the
//! extracted entity model layers (cubes baked on the CPU), the textures are
//! added to the sheet of the item atlas; the item's transformation (`transformation`
//! in the JSON) then the display of the base model are applied by the atlas.

use hashbrown::HashMap;

use glam::{Mat4, Vec3};
use rc_assets::texture::Image;
use rc_data::{EntityCube, EntityLayer};
use rc_proto::Reader;
use rc_proto::varint::read_varint;

use crate::block_entities::{DYES, dye_rgb};

/// Textures and layers of the special models, loaded at startup.
#[derive(Default)]
pub struct SpecialAssets {
    /// Images (added to the item atlas sheet), by name (`entity/banner/base`).
    pub images: Vec<Image>,
    index: HashMap<String, usize>,
    layers: HashMap<String, EntityLayer>,
    /// Names of the images, in the order of `images` (to find their place in the sheet of the
    /// entities).
    pub names: Vec<String>,
    /// Sherd pattern of each item of the registry (decorated pot), by item id.
    pub sherds: Vec<Option<String>>,
}

/// `textures` property of the player heads seen, by head identity (`head_data`): read by the
/// entity renderer to request the skin.
static HEADS: std::sync::Mutex<Option<HashMap<u128, String>>> = std::sync::Mutex::new(None);

/// Remembered `textures` property of a head identity.
pub fn head_textures(key: u128) -> Option<String> {
    HEADS.lock().ok()?.as_ref()?.get(&key).cloned()
}

/// `profile` component read: (id, username, `textures` property, complete profile, number of
/// properties, texture of the skin patch).
#[allow(clippy::type_complexity)]
fn parse_profile(b: &[u8]) -> Option<(Option<u128>, Option<String>, Option<String>, bool, usize, Option<String>)> {
    let mut r = Reader::new(b);
    let mut uuid = None;
    let mut name = None;
    let full = r.u8().ok()? == 1;
    if full {
        uuid = Some(u128::from_be_bytes(r.take_array().ok()?));
        name = Some(rc_proto::read_str(&mut r, 16).ok()?.to_owned());
    } else {
        if r.u8().ok()? == 1 {
            name = Some(rc_proto::read_str(&mut r, 16).ok()?.to_owned());
        }
        if r.u8().ok()? == 1 {
            uuid = Some(u128::from_be_bytes(r.take_array().ok()?));
        }
    }
    let n = rc_proto::read_len(&mut r, 16).ok()?;
    let mut textures = None;
    for _ in 0..n {
        let name = rc_proto::read_str(&mut r, 64).ok()?;
        let value = rc_proto::read_str(&mut r, 32767).ok()?;
        if r.u8().ok()? == 1 {
            rc_proto::read_str(&mut r, 1024).ok()?;
        }
        if name == "textures" {
            textures = Some(value.to_owned());
        }
    }
    // Skin patch: texture (resource id), cape, elytra, model, each one
    // optional; only the texture matters for a head. Absent (old format): none.
    let patch = match r.u8() {
        Ok(1) => Some(rc_proto::read_str(&mut r, 32767).ok()?.to_owned()),
        _ => None,
    };
    Some((uuid, name, textures, full, n, patch))
}

/// "Dynamic" profile: reduced to a username or an id (only one of the two), without
/// property; resolved later by the profile service.
fn dynamic(full: bool, props: usize, uuid: Option<u128>, name: Option<&str>) -> bool {
    !full && props == 0 && uuid.is_some() != name.is_some()
}

/// The `profile` component is a dynamic profile (dedicated tooltip line).
pub fn profile_dynamic(b: &[u8]) -> bool {
    parse_profile(b).is_some_and(|(uuid, name, _, full, props, _)| dynamic(full, props, uuid, name.as_deref()))
}

/// Data of a player head: identity (4 words, high word first) if the profile carries the
/// `textures` property, otherwise nothing (default skin). Identity = UUID of the profile, otherwise
/// fingerprint of the property (like placed heads).
pub fn head_data(profile: Option<&[u8]>) -> Vec<u32> {
    let Some(b) = profile else { return Vec::new() };
    let Some((uuid, name, textures, full, props, patch)) = parse_profile(b) else { return Vec::new() };
    let hash = rc_text::head_hash;
    // Complete profile or one with properties: skin from the `textures` property. Profile reduced to a username
    // or an id, without property: resolved by the profile service (like the game).
    let (key, spec) = match (patch, textures) {
        // Texture forced by the patch: it replaces the profile's skin (game resource).
        (Some(id), _) => {
            let spec = format!("asset:{id}");
            (hash(&spec), spec)
        }
        (None, Some(t)) => (uuid.unwrap_or_else(|| hash(&t)), t),
        (None, None) if dynamic(full, props, uuid, name.as_deref()) => match (&name, uuid) {
            (Some(n), _) => (hash(&format!("name:{}", n.to_lowercase())), format!("name:{n}")),
            (_, Some(u)) => (u, format!("id:{u:032x}")),
            _ => return Vec::new(),
        },
        // Profile without textures: default skin drawn from its id (like the game).
        (None, None) => {
            let Some(u) = uuid else { return Vec::new() };
            if let Ok(mut d) = DEFAULT_HEADS.lock()
                && !d.iter().any(|h| h.0 == u)
            {
                d.push((u, crate::entity_render::default_skin(u)));
            }
            return (0..4).map(|i| (u >> (96 - 32 * i)) as u32).collect();
        }
    };
    let fresh = HEADS.lock().is_ok_and(|mut g| g.get_or_insert_with(HashMap::new).insert(key, spec.clone()).is_none());
    // Skin request for the interface icon (world rendering goes through `head_textures`).
    if fresh && let Ok(mut w) = WANTED.lock() {
        w.push((key, spec));
    }
    (0..4).map(|i| (key >> (96 - 32 * i)) as u32).collect()
}

/// Skins requested by item icons (head identity, property or profile to resolve),
/// emptied by the main loop.
static WANTED: std::sync::Mutex<Vec<(u128, String)>> = std::sync::Mutex::new(Vec::new());

/// Heads whose profile has no textures: (identity, default skin of the UUID), emptied by
/// the main loop which sends the default image to the icon atlas.
static DEFAULT_HEADS: std::sync::Mutex<Vec<(u128, u8)>> = std::sync::Mutex::new(Vec::new());

/// Heads without textures to dress in their default skin (once each).
pub fn take_default_heads() -> Vec<(u128, u8)> {
    DEFAULT_HEADS.lock().map(|mut w| std::mem::take(&mut *w)).unwrap_or_default()
}

/// Default skin (64 × 64) of an index of `default_skin`.
pub fn default_skin_image(assets: &rc_assets::Assets, skin: u8) -> Option<Image> {
    let kind = if skin < 9 { "slim" } else { "wide" };
    assets.image(&format!(
        "assets/minecraft/textures/entity/player/{kind}/{}.png",
        crate::entity_render::SKINS[skin as usize % 9]
    ))
}

/// Skin requests pending (once each).
pub fn take_wanted() -> Vec<(u128, String)> {
    WANTED.lock().map(|mut w| std::mem::take(&mut *w)).unwrap_or_default()
}

/// Head identity rebuilt from the `head_data` data.
pub fn head_key_of(data: &[u32]) -> Option<u128> {
    (data.len() == 4).then(|| data.iter().fold(0u128, |a, &w| a << 32 | u128::from(w)))
}

const SKULLS: [(&str, &str); 7] = [
    ("minecraft:skeleton_skull#main", "entity/skeleton/skeleton"),
    ("minecraft:wither_skeleton_skull#main", "entity/skeleton/wither_skeleton"),
    ("minecraft:zombie_head#main", "entity/zombie/zombie"),
    ("minecraft:creeper_head#main", "entity/creeper/creeper"),
    ("minecraft:dragon_skull#main", "entity/enderdragon/dragon"),
    ("minecraft:piglin_head#main", "entity/piglin/piglin"),
    ("minecraft:player_head#main", "entity/player/wide/steve"),
];



/// Textures of the copper golem by oxidation level.
const GOLEMS: [&str; 4] = [
    "entity/copper_golem/copper_golem",
    "entity/copper_golem/copper_golem_exposed",
    "entity/copper_golem/copper_golem_weathered",
    "entity/copper_golem/copper_golem_oxidized",
];

const LAYERS: [&str; 13] = [
    "minecraft:standing_banner#main",
    "minecraft:standing_banner#flag",
    "minecraft:shield#main",
    "minecraft:chest#main",
    "minecraft:shulker_box#main",
    "minecraft:conduit#shell",
    "minecraft:trident#main",
    "minecraft:decorated_pot_base#main",
    "minecraft:decorated_pot_sides#main",
    "minecraft:copper_golem#main",
    "minecraft:copper_golem_running#main",
    "minecraft:copper_golem_sitting#main",
    "minecraft:copper_golem_star#main",
];

impl SpecialAssets {
    pub fn load(assets: &rc_assets::Assets, data: &rc_data::VersionData) -> SpecialAssets {
        let mut names: Vec<String> = SKULLS.iter().map(|s| s.1.to_owned()).collect();
        names.extend(rc_assets::item::CHEST_TEXTURES.iter().map(|s| format!("entity/chest/{s}")));
        names.push("entity/shulker/shulker".to_owned());
        names.extend(DYES.iter().map(|d| format!("entity/shulker/shulker_{d}")));
        names.extend(["entity/trident/trident".to_owned(), "entity/conduit/base".to_owned()]);
        names.extend(GOLEMS.iter().map(|g| (*g).to_owned()));
        for dir in ["banner", "shield", "decorated_pot"] {
            let prefix = format!("assets/minecraft/textures/entity/{dir}/");
            for p in assets.list(&prefix, ".png") {
                if let Some(n) = p.strip_prefix("assets/minecraft/textures/").and_then(|n| n.strip_suffix(".png")) {
                    names.push(n.to_owned());
                }
            }
        }
        let sherds = data
            .registry("minecraft:item")
            .map(|r| {
                r.entries
                    .iter()
                    .map(|n| {
                        n.strip_prefix("minecraft:").and_then(|n| n.strip_suffix("_pottery_sherd")).map(str::to_owned)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut s = SpecialAssets { sherds, ..Default::default() };
        for n in names {
            if s.index.contains_key(&n) {
                continue;
            }
            if let Some(img) = rc_assets::texture::load(assets, &format!("minecraft:{n}")) {
                s.index.insert(n.clone(), s.images.len());
                s.names.push(n);
                s.images.push(img);
            }
        }
        for l in SKULLS.iter().map(|s| s.0).chain(LAYERS) {
            if let Some(layer) = data.entity_layer(l) {
                s.layers.insert(l.to_owned(), layer.clone());
            }
        }
        s
    }
}

impl SpecialAssets {
    /// Size (pixels) of the player head layer's texture: the quads' UVs are
    /// normalised there, so a downloaded skin (64 × 64) must be read over that extent.
    pub fn head_texture_size(&self) -> [f32; 2] {
        self.layers.get(SKULLS[6].0).map_or([64.0, 64.0], |l| [f32::from(l.texture[0]), f32::from(l.texture[1])])
    }

    /// Copy without the images (already gone into the icon atlas): layers and names only,
    /// for rendering the special models in the world.
    pub fn geometry_only(&self) -> SpecialAssets {
        SpecialAssets {
            images: Vec::new(),
            index: self.index.clone(),
            layers: self.layers.clone(),
            names: self.names.clone(),
            sherds: self.sherds.clone(),
        }
    }
}

/// Quad of a special model (item space): vertices, UV in the image, normal, image,
/// ARGB tint.
pub struct SQuad {
    pub pos: [[f32; 3]; 4],
    pub uv: [[f32; 2]; 4],
    pub normal: [f32; 3],
    pub image: usize,
    pub tint: u32,
}

/// Baked face: vertices (model pixels), UV, normal.
type Face = ([[f32; 3]; 4], [[f32; 2]; 4], [f32; 3]);

/// Faces of a cube (`entities.md` part 2 § 1.4), in model pixels.
fn cube(c: &EntityCube, tex: [u16; 2], out: &mut Vec<Face>) {
    let [x0, y0, z0] = c.from;
    let [w, h, d] = c.size;
    let [gx, gy, gz] = c.grow;
    let (mut xa, mut xb) = (x0 - gx, x0 + w + gx);
    let (ya, yb) = (y0 - gy, y0 + h + gy);
    let (za, zb) = (z0 - gz, z0 + d + gz);
    if c.mirror {
        std::mem::swap(&mut xa, &mut xb);
    }
    let t = [[xa, ya, za], [xb, ya, za], [xb, yb, za], [xa, yb, za]];
    let l = [[xa, ya, zb], [xb, ya, zb], [xb, yb, zb], [xa, yb, zb]];
    let (u, v) = (c.uv[0].trunc(), c.uv[1].trunc());
    let (u0, u1, u2, u22, u3, u4) = (u, u + d, u + d + w, u + d + w + w, u + d + w + d, u + d + w + d + w);
    let (v0, v1, v2) = (v, v + d, v + d + h);
    let tw = f32::from(tex[0]) * c.tex_scale[0];
    let th = f32::from(tex[1]) * c.tex_scale[1];
    #[allow(clippy::type_complexity)]
    let faces: [(u8, [[f32; 3]; 4], [f32; 4], [f32; 3]); 6] = [
        (0, [l[1], l[0], t[0], t[1]], [u1, v0, u2, v1], [0.0, -1.0, 0.0]),
        (1, [t[2], t[3], l[3], l[2]], [u2, v1, u22, v0], [0.0, 1.0, 0.0]),
        (4, [t[0], l[0], l[3], t[3]], [u0, v1, u1, v2], [-1.0, 0.0, 0.0]),
        (2, [t[1], t[0], t[3], t[2]], [u1, v1, u2, v2], [0.0, 0.0, -1.0]),
        (5, [l[1], t[1], t[2], l[2]], [u2, v1, u3, v2], [1.0, 0.0, 0.0]),
        (3, [l[0], l[1], l[2], l[3]], [u3, v1, u4, v2], [0.0, 0.0, 1.0]),
    ];
    for (bit, mut pos, r, n) in faces {
        if c.faces & (1 << bit) == 0 {
            continue;
        }
        let mut uv = [[r[2] / tw, r[1] / th], [r[0] / tw, r[1] / th], [r[0] / tw, r[3] / th], [r[2] / tw, r[3] / th]];
        let mut normal = n;
        if c.mirror {
            pos.reverse();
            uv.reverse();
            if n[0] != 0.0 {
                normal[0] = -n[0];
            }
        }
        out.push((pos, uv, normal));
    }
}

/// Matrix of a part: T · Rz · Ry · Rx · S (translation in pixels).
fn part_matrix(p: &[f32; 9]) -> Mat4 {
    Mat4::from_translation(Vec3::new(p[0] / 16.0, p[1] / 16.0, p[2] / 16.0))
        * Mat4::from_rotation_z(p[5])
        * Mat4::from_rotation_y(p[4])
        * Mat4::from_rotation_x(p[3])
        * Mat4::from_scale(Vec3::new(p[6], p[7], p[8]))
}

impl SpecialAssets {
    /// Quads of a layer with the image `image` and the tint `tint`; `pose` adjusts a part.
    fn layer(&self, name: &str, image: &str, tint: u32, pose: &dyn Fn(&str, &mut [f32; 9]), out: &mut Vec<SQuad>) {
        let (Some(l), Some(&image)) = (self.layers.get(name), self.index.get(image)) else { return };
        let mut world: Vec<Mat4> = Vec::with_capacity(l.parts.len());
        let mut faces = Vec::new();
        for p in &l.parts {
            let mut q = p.pose;
            pose(&p.name, &mut q);
            let parent = if p.parent == u32::MAX { Mat4::IDENTITY } else { world[p.parent as usize] };
            let m = parent * part_matrix(&q);
            world.push(m);
            faces.clear();
            for c in &p.cubes {
                cube(c, l.texture, &mut faces);
            }
            let rot = Mat4::from_mat3(glam::Mat3::from_mat4(m));
            for (pos, uv, n) in &faces {
                let pos = pos.map(|v| m.transform_point3(Vec3::from(v) / 16.0).to_array());
                let normal = rot.transform_vector3(Vec3::from(*n)).normalize_or_zero().to_array();
                out.push(SQuad { pos, uv: *uv, normal, image, tint });
            }
        }
    }

    /// Geometry of the special model `kind` (index + 1 in `SPECIAL_KINDS`) and of its data.
    pub fn quads(&self, kind: u16, data: &[u32], out: &mut Vec<SQuad>) {
        let white = 0xffff_ffff;
        let none = |_: &str, _: &mut [f32; 9]| {};
        match kind {
            // Head (kind 0..5) or player head (default skin).
            1 | 2 => {
                let k = if kind == 2 { 6 } else { data.first().map_or(0, |&k| (k as usize).min(5)) };
                let (layer, image) = SKULLS[k];
                self.layer(layer, image, white, &none, out);
            }
            // Banner: pole and flag (background), base colour, patterns.
            3 => {
                let flag_x = (-0.0125 + 0.01) * std::f32::consts::PI;
                let wave = move |n: &str, q: &mut [f32; 9]| {
                    if n == "flag" {
                        q[3] = flag_x;
                    }
                };
                self.layer("minecraft:standing_banner#main", "entity/banner/banner_base", white, &none, out);
                self.layer("minecraft:standing_banner#flag", "entity/banner/banner_base", white, &wave, out);
                let base = data.first().copied().unwrap_or(0);
                self.layer("minecraft:standing_banner#flag", "entity/banner/base", dye(base), &wave, out);
                for (p, c) in patterns(data.get(1..).unwrap_or(&[])) {
                    self.layer("minecraft:standing_banner#flag", &format!("entity/banner/{p}"), dye(c), &wave, out);
                }
            }
            // Shield: background (without pattern: own texture), base colour, patterns.
            4 => {
                let base = data.first().copied().unwrap_or(16);
                let pats = patterns(data.get(1..).unwrap_or(&[]));
                if base >= 16 && pats.is_empty() {
                    self.layer("minecraft:shield#main", "entity/shield/shield_base_nopattern", white, &none, out);
                    return;
                }
                self.layer("minecraft:shield#main", "entity/shield/shield_base", white, &none, out);
                if base < 16 {
                    self.layer("minecraft:shield#main", "entity/shield/base", dye(base), &none, out);
                }
                for (p, c) in pats {
                    self.layer("minecraft:shield#main", &format!("entity/shield/{p}"), dye(c), &none, out);
                }
            }
            // Chest: texture fixed by the definition (index of CHEST_TEXTURES).
            5 => {
                let names = &rc_assets::item::CHEST_TEXTURES;
                let k = data.first().map_or(0, |&k| (k as usize).min(names.len() - 1));
                self.layer("minecraft:chest#main", &format!("entity/chest/{}", names[k]), white, &none, out);
            }
            6 => {
                let c = data.first().copied().unwrap_or(16);
                let image = if c < 16 {
                    format!("entity/shulker/shulker_{}", DYES[c as usize])
                } else {
                    "entity/shulker/shulker".into()
                };
                self.layer("minecraft:shulker_box#main", &image, white, &none, out);
            }
            7 => self.layer("minecraft:conduit#shell", "entity/conduit/base", white, &none, out),
            8 => self.layer("minecraft:trident#main", "entity/trident/trident", white, &none, out),
            // Decorated pot: base (neck, top, bottom), then each face (back, left, right, front)
            // with the pattern of its sherd; data = item ids + 1 (0: plain face).
            9 => {
                self.layer(
                    "minecraft:decorated_pot_base#main",
                    "entity/decorated_pot/decorated_pot_base",
                    white,
                    &none,
                    out,
                );
                const SIDES: [&str; 4] = ["back", "left", "right", "front"];
                for (i, side) in SIDES.into_iter().enumerate() {
                    let sherd = data
                        .get(i)
                        .and_then(|&id| id.checked_sub(1))
                        .and_then(|id| self.sherds.get(id as usize))
                        .and_then(Option::as_deref);
                    let image = match sherd {
                        Some(p) => format!("entity/decorated_pot/{p}_pottery_pattern"),
                        None => "entity/decorated_pot/decorated_pot_side".to_owned(),
                    };
                    let only = move |n: &str, q: &mut [f32; 9]| {
                        if SIDES.contains(&n) && n != side {
                            q[6..9].fill(0.0);
                        }
                    };
                    self.layer("minecraft:decorated_pot_sides#main", &image, white, &only, out);
                }
            }
            // Copper golem statue: pose then oxidation (root laid flat, like the
            // statue model).
            10 => {
                let pose = data.first().map_or(0, |&p| (p as usize).min(3));
                let ox = data.get(1).map_or(0, |&o| (o as usize).min(3));
                let layer = [
                    "minecraft:copper_golem#main",
                    "minecraft:copper_golem_running#main",
                    "minecraft:copper_golem_sitting#main",
                    "minecraft:copper_golem_star#main",
                ][pose];
                let root = |n: &str, q: &mut [f32; 9]| {
                    if n == "root" {
                        q[1] = 0.0;
                        q[5] = std::f32::consts::PI;
                    }
                };
                self.layer(layer, GOLEMS[ox], white, &root, out);
            }
            _ => {}
        }
    }
}

fn dye(d: u32) -> u32 {
    let [r, g, b] = dye_rgb(d.min(15) as u8);
    0xff00_0000 | u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
}

/// Patterns encoded `n, (id + 1, colour)…` → (texture name, colour), 16 at most.
fn patterns(d: &[u32]) -> Vec<(String, u32)> {
    let n = d.first().copied().unwrap_or(0) as usize;
    let list = crate::screens_work::PATTERNS.read().unwrap_or_else(|p| p.into_inner());
    (0..n.min(16))
        .filter_map(|i| {
            let (id, c) = (*d.get(1 + 2 * i)?, *d.get(2 + 2 * i)?);
            let asset = &list.get(id.checked_sub(1)? as usize)?.1;
            Some((asset.strip_prefix("minecraft:").unwrap_or(asset).to_owned(), c))
        })
        .collect()
}

/// Icon data of a stack (see `StackView::special_data`): item name without
/// namespace, bytes of the `banner_patterns` and `base_color` components.
pub fn data(kind: &str, name: &str, patterns: Option<&[u8]>, base_color: Option<&[u8]>) -> Vec<u32> {
    let dye_of = |prefix: &str| DYES.iter().position(|d| *d == prefix).map(|i| i as u32);
    match kind {
        "minecraft:head" => {
            let k = [
                "skeleton_skull",
                "wither_skeleton_skull",
                "zombie_head",
                "creeper_head",
                "dragon_head",
                "piglin_head",
            ]
            .iter()
            .position(|n| *n == name)
            .unwrap_or(0);
            vec![k as u32]
        }
        "minecraft:banner" | "minecraft:shield" => {
            let base = if kind == "minecraft:banner" {
                dye_of(name.strip_suffix("_banner").unwrap_or("white")).unwrap_or(0)
            } else {
                base_color.and_then(|b| read_varint(&mut Reader::new(b)).ok()).map_or(16, |v| v as u32)
            };
            let mut out = vec![base];
            let mut list = Vec::new();
            if let Some(b) = patterns {
                let mut r = Reader::new(b);
                let n = read_varint(&mut r).unwrap_or(0);
                for _ in 0..n {
                    let Ok(h) = read_varint(&mut r) else { break };
                    if h == 0 {
                        // Pattern written inline: not resolved (rare), read skipped.
                        let (Ok(_), Ok(_)) = (rc_proto::read_str(&mut r, 32767), rc_proto::read_str(&mut r, 32767))
                        else {
                            break;
                        };
                        let _ = read_varint(&mut r);
                        continue;
                    }
                    let Ok(c) = read_varint(&mut r) else { break };
                    list.push((h as u32, c as u32));
                }
            }
            out.push(list.len() as u32);
            out.extend(list.into_iter().flat_map(|(h, c)| [h, c]));
            out
        }

        "minecraft:shulker_box" => vec![name.strip_suffix("_shulker_box").and_then(dye_of).unwrap_or(16)],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tests that read the global queue of requested skins empty it: only one at a time,
    /// otherwise one takes the request expected by the other (parallel test threads).
    fn wanted_lock() -> std::sync::MutexGuard<'static, ()> {
        static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
        L.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// "Dynamic profile" tooltip: username alone or id alone, without property.
    #[test]
    fn dynamic_profiles() {
        let partial = |name: Option<&str>, uuid: Option<u128>, props: u8| {
            let mut b = vec![0];
            match name {
                Some(n) => {
                    b.push(1);
                    rc_proto::write_str(&mut b, n);
                }
                None => b.push(0),
            }
            match uuid {
                Some(u) => {
                    b.push(1);
                    b.extend_from_slice(&u.to_be_bytes());
                }
                None => b.push(0),
            }
            b.push(props);
            for _ in 0..props {
                rc_proto::write_str(&mut b, "textures");
                rc_proto::write_str(&mut b, "v");
                b.push(0);
            }
            b
        };
        assert!(profile_dynamic(&partial(Some("Steve"), None, 0)));
        assert!(profile_dynamic(&partial(None, Some(7), 0)));
        assert!(!profile_dynamic(&partial(Some("Steve"), Some(7), 0)));
        assert!(!profile_dynamic(&partial(Some("Steve"), None, 1)));
        assert!(!profile_dynamic(&partial(None, None, 0)));
        let mut full = vec![1];
        full.extend_from_slice(&7u128.to_be_bytes());
        rc_proto::write_str(&mut full, "Steve");
        full.push(0);
        assert!(!profile_dynamic(&full));
    }

    /// Profile reduced to a username: head identity and `name:…` resolution request.
    #[test]
    fn name_only_profile_is_resolved_by_service() {
        let _q = wanted_lock();
        // Not complete, username "Notch", no id, no property.
        let b = [0u8, 1, 5, b'N', b'o', b't', b'c', b'h', 0, 0];
        let d = head_data(Some(&b));
        assert_eq!(d.len(), 4);
        let key = head_key_of(&d).expect("identity");
        assert_eq!(head_textures(key).as_deref(), Some("name:Notch"));
        assert!(take_wanted().iter().any(|(k, s)| *k == key && s == "name:Notch"));
    }

    /// Skin patch of the profile: its texture replaces the skin (game resource, never the
    /// profile service), identity specific to the texture.
    #[test]
    fn skin_patch_forces_the_texture() {
        let _q = wanted_lock();
        let id = b"minecraft:entity/player/wide/alex";
        let mut b = vec![0u8, 1, 5, b'P', b'a', b't', b'c', b'h', 0, 0, 1, id.len() as u8];
        b.extend(id);
        b.extend([0, 0, 0]);
        let d = head_data(Some(&b));
        let key = head_key_of(&d).expect("identity");
        assert_eq!(head_textures(key).as_deref(), Some("asset:minecraft:entity/player/wide/alex"));
        assert!(take_wanted().iter().any(|(k, s)| *k == key && s == "asset:minecraft:entity/player/wide/alex"));
        // Same profile without patch: resolved by username.
        let plain = head_data(Some(&b[..10]));
        assert_ne!(head_key_of(&plain), Some(key));
    }

    /// Complete profile without `textures` property: default skin of the UUID, nothing to request
    /// from the profile service.
    #[test]
    fn full_profile_without_textures_gets_uuid_default() {
        let _q = wanted_lock();
        let mut b = vec![1u8];
        b.extend([0u8; 15]);
        b.push(7);
        b.extend([3, b'a', b'b', b'c', 0]);
        let d = head_data(Some(&b));
        assert_eq!(head_key_of(&d), Some(7));
        assert!(take_wanted().iter().all(|(k, _)| *k != 7));
        assert!(take_default_heads().contains(&(7, crate::entity_render::default_skin(7))));
    }
}
