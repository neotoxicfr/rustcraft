//! Entity rendering (`docs/specs/entities.md` parts 2 and 3): cubes baked bit-exact from
//! the extracted layers, animations of the common models (humanoids, quadrupeds, chicken,
//! spider, creeper, sheep), vertices transformed on the CPU, one pipeline per render type
//! sharing the camera, lightmap and fog of the terrain.

use hashbrown::HashMap;
use std::f32::consts::PI;
use std::sync::Arc;
use std::time::Instant;

use glam::{Mat4, Vec3};
use rc_assets::texture::Image;
use rc_data::{EntityLayer, VersionData};

use crate::entities::{Entity, Kind, Size, rot_lerp};

// Renderers per family (`docs/specs/entities-animals.md`): each module exposes the same entry
// points (`layers`, `skin`, `animate`, `rotations`, `scale`, `shadow`, `tick`), consulted before
// the rules of this file.
mod animals;
// Block entities (`docs/specs/block-entities.md`): chests, signs, banners, beams…
mod blocks;
pub use blocks::BlockFrame;
mod creatures;
// Transient blocks (`docs/specs/miscellaneous.md` § 5.5).
mod misc;
mod dragon;
mod keyframes;
mod wither;
// Maps (`docs/specs/maps.md` § 6): pixels, decorations, background in hand.
mod maps;
pub(crate) use maps::map_label_matrix;
mod shulker;
// Humanoid models, held items, illusions (mission G2-F03).
#[cfg(test)]
mod tests_g1f08;
#[cfg(test)]
mod tests_g2f03;
// Entity objects and maps (mission G2-F05).
#[cfg(test)]
mod tests_g2f05;
#[cfg(test)]
mod tests_c3m11;
// Non-living entities and projectiles (`docs/specs/entities-objects.md`): complete rendering outside the
// animated-models path (`push_object`), block models inside an entity.
mod objects;
mod transparency;
pub(crate) use objects::{DisplayState, display_cull_box, display_data, display_tick};
pub(crate) use objects::{DisplaySub, ObjectsFrame, TextDraw};
use transparency::EntityOit;

/// Client tick specific to the families (free state `Entity::extra`).
pub fn tick_extra(e: &mut Entity) {
    tick_pose(e);
    animals::tick(e);
    creatures::tick(e);
    dragon::tick(e);
    objects::tick(e);
    shulker::tick(e);
}

/// New direct position of an entity (old position): start of the shulker's slide.
pub fn shulker_teleported(e: &mut Entity, old: [f64; 3]) {
    shulker::teleported(e, old);
}

/// Mounting (`true`) or dismounting a mount: shulker slide and body.
pub fn shulker_riding(e: &mut Entity, mounted: bool) {
    shulker::riding_changed(e, mounted);
}

/// After the entity tick, what depends on neighbours: aiming of the heads of each wither
/// (targets by network id), cat lying on a sleeping player.
pub fn wither_track(ents: &mut crate::entities::Entities) {
    wither::track(ents);
    animals::cat_track(ents);
}

/// Jukebox that starts or stops playing (events 1010 / 1011): nearby parrots marked.
pub fn jukebox_notify(ents: &mut crate::entities::Entities, dims: &[[f32; 4]], p: [i32; 3], playing: bool) {
    animals::jukebox_notify(ents, dims, p, playing);
}

/// Before the entity tick: parrots too far from their jukebox (or jukebox gone) unmarked.
pub fn party_track(ents: &mut crate::entities::Entities, is_jukebox: &dyn Fn([i32; 3]) -> bool) {
    animals::party_track(ents, is_jukebox);
}

/// Water contact of the kinds whose pose depends on it (frog, turtle, equines), noted
/// before their tick like the game; nothing is computed for the others.
pub fn water_track(
    ents: &mut crate::entities::Entities,
    data: &rc_data::VersionData,
    dims: &[[f32; 4]],
    state: &dyn Fn(i32, i32, i32) -> u32,
) {
    let mask = water_fluids(data);
    for e in ents.map.values_mut() {
        if animals::water_posed(&e.kind) {
            e.in_water = touches_water(data, dims, e, state, mask);
        }
    }
}

/// Bits of the water fluids (indices of `fluids`), read once per call rather than per block.
fn water_fluids(data: &rc_data::VersionData) -> u64 {
    data.fluids.iter().enumerate().filter(|(_, n)| n.contains("water")).fold(0, |m, (i, _)| m | 1 << (i & 63))
}

/// Entity box (shrunk by 0.001) in contact with water: water block whose
/// surface (full under water, otherwise level / 9) reaches the bottom of the box. `mask`:
/// `water_fluids`.
fn touches_water(
    data: &rc_data::VersionData,
    dims: &[[f32; 4]],
    e: &Entity,
    state: &dyn Fn(i32, i32, i32) -> u32,
    mask: u64,
) -> bool {
    let s = e.size(dims);
    let half = f64::from(s.w) / 2.0 - 0.001;
    let lo = [e.pos[0] - half, e.pos[1] + 0.001, e.pos[2] - half];
    let hi = [e.pos[0] + half, e.pos[1] + f64::from(s.h) - 0.001, e.pos[2] + half];
    let water = |st: u32| -> Option<u8> {
        let info = data.state(st)?;
        (mask >> (info.fluid & 63) & 1 != 0).then_some(info.fluid_amount)
    };
    for x in lo[0].floor() as i32..=hi[0].floor() as i32 {
        for z in lo[2].floor() as i32..=hi[2].floor() as i32 {
            for y in lo[1].floor() as i32..=hi[1].floor() as i32 {
                let Some(amount) = water(state(x, y, z)) else { continue };
                let full = water(state(x, y + 1, z)).is_some();
                let h = if full { 1.0 } else { f64::from(amount) / 9.0 };
                if f64::from(y) + h >= lo[1] {
                    return true;
                }
            }
        }
    }
    false
}

/// Client-side thunderstorm (world with weather and thunder > 0.9), for the worried panda.
pub fn set_thundering(on: bool) {
    animals::set_thundering(on);
}

/// Target walk speed and smoothing factor depending on the kind (distance travelled per tick).
pub fn walk_target(e: &Entity, dist: f32) -> (f32, f32) {
    animals::walk_target(e, dist)
}

/// World time, for anger with a deadline (wolf, bee).
pub fn set_game_time(t: i64) {
    animals::set_game_time(t);
}

/// Entity event specific to the families (rabbit jump, squid tentacles); true if
/// handled.
pub fn event_extra(e: &mut Entity, id: i8) -> bool {
    animals::event(e, id) || creatures::event(e, id)
}

/// Poses tracked client-side for every living entity: swim progress, glide duration,
/// elytra opening, duration of the current use, movement of the previous tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PoseAnim {
    swim: f32,
    swim_o: f32,
    fly_ticks: i32,
    elytra: [f32; 3],
    elytra_o: [f32; 3],
    use_ticks: i32,
    using: bool,
    vel_o: [f64; 3],
}

impl PoseAnim {
    /// Swim progress (0 standing, 1 flat), interpolated.
    fn swim(&self, pt: f32) -> f32 {
        self.swim_o + pt * (self.swim - self.swim_o)
    }

    /// Glide opening: square of the flight ticks / 100, capped at 1.
    fn fly_scale(&self, pt: f32) -> f32 {
        let t = self.fly_ticks as f32 + pt;
        (t * t / 100.0).clamp(0.0, 1.0)
    }

    /// Wing angles (x, y, z), interpolated.
    fn elytra(&self, pt: f32) -> [f32; 3] {
        std::array::from_fn(|i| self.elytra_o[i] + pt * (self.elytra[i] - self.elytra_o[i]))
    }

    /// Use ticks of the current item, interpolated (0 without use).
    fn use_ticks(&self, pt: f32) -> f32 {
        if self.using { self.use_ticks as f32 + pt } else { 0.0 }
    }
}

/// World context read at render for the poses: bed (step x, step z, yaw in degrees of the
/// bed direction) and water contact.
#[derive(Clone, Copy, Debug, Default)]
struct PoseEnv {
    bed: Option<(f32, f32, f32)>,
    in_water: bool,
}

/// Glide yaw (radians) towards the movement direction: signed angle between the
/// horizontal movement (interpolated from the previous tick) and the horizontal gaze; none
/// if either is almost zero.
fn flying_y_rot(e: &Entity, pt: f32) -> Option<f32> {
    let pt64 = f64::from(pt);
    let a = &e.pose_anim;
    let mv: [f64; 3] = std::array::from_fn(|i| a.vel_o[i] + pt64 * (e.velocity[i] - a.vel_o[i]));
    let yaw = f64::from(e.yaw_o + pt * (e.yaw - e.yaw_o)).to_radians();
    let pitch = f64::from(e.pitch_o + pt * (e.pitch - e.pitch_o)).to_radians();
    let look = [-yaw.sin() * pitch.cos(), yaw.cos() * pitch.cos()];
    let m2 = mv[0] * mv[0] + mv[2] * mv[2];
    let l2 = look[0] * look[0] + look[1] * look[1];
    if m2 <= 1.0e-5 || l2 <= 1.0e-5 {
        return None;
    }
    let dot = (mv[0] * look[0] + mv[2] * look[1]) / (m2.sqrt() * l2.sqrt());
    let sign = mv[0] * look[1] - mv[2] * look[0];
    let s = if sign > 0.0 {
        1.0
    } else if sign < 0.0 {
        -1.0
    } else {
        0.0
    };
    Some((s * dot.abs().min(1.0).acos()) as f32)
}

/// Villager type hat visible (0 none, 1 partial, 2 full): profession without hat,
/// or profession with a partial hat on a type without a full hat.
fn type_hat_visible(type_hat: u8, prof_hat: u8) -> bool {
    prof_hat == 0 || (prof_hat == 1 && type_hat != 2)
}

/// Custom name (value 2) as plain text.
fn custom_name(e: &Entity) -> Option<std::borrow::Cow<'_, str>> {
    // Plain text kept on reception (`RUSTCRAFT_OPT_NAMECACHE`), otherwise parsed here.
    if crate::entities::name_cache_on() {
        return e.meta.custom_name().map(std::borrow::Cow::Borrowed);
    }
    crate::entities::parse_custom_name(e.meta.get(&2)).map(std::borrow::Cow::Owned)
}

/// Wool of the "jeb_" sheep: dye colours in order, 25 ticks each, integer linear
/// fade between two (the game's wool colours).
fn jeb_wool(age: f32) -> u32 {
    let ticks = age.floor() as i32;
    let value = ticks / 25;
    let (c1, c2) = (wool_color((value % 16) as usize), wool_color(((value + 1) % 16) as usize));
    let sub = ((ticks % 25) as f32 + age.fract()) / 25.0;
    let ch = |s: u32| {
        let (a, b) = ((c1 >> s & 0xff) as i32, (c2 >> s & 0xff) as i32);
        ((a + (sub * (b - a) as f32).floor() as i32) as u32) << s
    };
    0xff00_0000 | ch(16) | ch(8) | ch(0)
}

/// Visible swimming: "swim" pose, or "glide" pose without gliding; drowned: "swim" flag
/// (0x10) when not mounted.
fn visually_swimming(e: &Entity) -> bool {
    if e.kind == "drowned" {
        return e.flags() & 0x10 != 0 && e.vehicle.is_none();
    }
    let pose = e.pose();
    pose == 3 || (pose == 1 && e.flags() & (0x80u8 as i8) == 0)
}

/// One tick of the tracked poses (`PoseAnim`): swim ±0.09 per tick depending on the "swim" pose,
/// glide counter (flag 0x80), wings that stretch by 30% towards their target (flight:
/// depending on the dive; crouching; rest), use ticks since the start of the use.
fn tick_pose(e: &mut Entity) {
    if e.class == Kind::Other {
        return;
    }
    let pose = e.pose();
    let flying = e.flags() & (0x80u8 as i8) != 0;
    let using = e.meta_byte(8) & 1 != 0;
    let vel = e.velocity;
    let swimming = visually_swimming(e);
    let a = &mut e.pose_anim;
    a.swim_o = a.swim;
    a.swim = if swimming { (a.swim + 0.09).min(1.0) } else { (a.swim - 0.09).max(0.0) };
    a.fly_ticks = if flying { a.fly_ticks + 1 } else { 0 };
    a.elytra_o = a.elytra;
    let target = if flying {
        let mut ratio = 1.0f32;
        if vel[1] < 0.0 {
            let len = (vel[0] * vel[0] + vel[1] * vel[1] + vel[2] * vel[2]).sqrt();
            ratio = 1.0 - (-vel[1] / len).powf(1.5) as f32;
        }
        [PI / 12.0 + ratio * (PI / 9.0 - PI / 12.0), 0.0, -PI / 12.0 + ratio * (-PI / 2.0 + PI / 12.0)]
    } else if pose == 5 {
        [PI * 2.0 / 9.0, 0.087_266_46, -PI / 4.0]
    } else {
        [PI / 12.0, 0.0, -PI / 12.0]
    };
    for (r, t) in a.elytra.iter_mut().zip(target) {
        *r += (t - *r) * 0.3;
    }
    a.use_ticks = if !using {
        0
    } else if a.using {
        a.use_ticks + 1
    } else {
        1
    };
    a.using = using;
    a.vel_o = vel;
}

/// Static vertex of a baked layer: position in pixels of the part's frame, UV
/// normalised to the layer, normal, part number.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct StaticVertex {
    pos: [f32; 3],
    uv: [f32; 2],
    normal: [f32; 3],
    part: u32,
}

/// Free-geometry vertex (beams, rays), recomputed every frame: position in the
/// camera frame (blocks), local UV, texture rectangle (pixels), RGBA colour, light,
/// normal.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FreeVertex {
    pos: [f32; 3],
    uv: [f32; 2],
    rect: [f32; 4],
    color: u32,
    light: u32,
    normal: [f32; 3],
}

/// Free-geometry lists: beams (cutout), rays (additive).
const FREE_BEAM: usize = 0;
const FREE_RAYS: usize = 1;
/// Guardian rays (per-face lighting); last in the concatenation.
const FREE_GBEAM: usize = 2;

/// Experience orb quad (model pixels, from (−0.5, −0.25) to (0.5, 0.75), up
/// normal), at the head of the object buffer.
fn orb_quad() -> Vec<StaticVertex> {
    let v = |x: f32, y: f32, u: f32, w: f32| StaticVertex {
        pos: [x * 16.0, y * 16.0, 0.0],
        uv: [u, w],
        normal: [0.0, 1.0, 0.0],
        part: 0,
    };
    let q = [v(-0.5, -0.25, 0.0, 1.0), v(0.5, -0.25, 1.0, 1.0), v(0.5, 0.75, 1.0, 0.0), v(-0.5, 0.75, 0.0, 0.0)];
    [0, 1, 2, 0, 2, 3].iter().map(|&i| q[i]).collect()
}

/// Painting tile of one block (model pixels, bottom-left corner at the origin): front face at
/// z = −1/32 (flipped UV: u = 1 − x, v = 1 − y), then back at z = +1/32 and four 1/16
/// slices (sprite `back`).
fn painting_tile() -> Vec<StaticVertex> {
    const Z: f32 = 0.5;
    let v = |p: [f32; 3], uv: [f32; 2], n: [f32; 3]| StaticVertex { pos: p.map(|c| c * 1.0), uv, normal: n, part: 0 };
    let quad = |q: [StaticVertex; 4]| [0, 1, 2, 0, 2, 3].map(|i| q[i]);
    let s = 16.0;
    let mut out = Vec::with_capacity(36);
    // Front (−Z normal).
    out.extend(quad([
        v([0.0, 0.0, -Z], [1.0, 1.0], [0.0, 0.0, -1.0]),
        v([0.0, s, -Z], [1.0, 0.0], [0.0, 0.0, -1.0]),
        v([s, s, -Z], [0.0, 0.0], [0.0, 0.0, -1.0]),
        v([s, 0.0, -Z], [0.0, 1.0], [0.0, 0.0, -1.0]),
    ]));
    // Back (+Z normal), whole sprite.
    out.extend(quad([
        v([0.0, 0.0, Z], [0.0, 1.0], [0.0, 0.0, 1.0]),
        v([s, 0.0, Z], [1.0, 1.0], [0.0, 0.0, 1.0]),
        v([s, s, Z], [1.0, 0.0], [0.0, 0.0, 1.0]),
        v([0.0, s, Z], [0.0, 0.0], [0.0, 0.0, 1.0]),
    ]));
    let b = 1.0 / 16.0;
    // Top and bottom: strip of 1/16 height.
    out.extend(quad([
        v([0.0, s, -Z], [0.0, 0.0], [0.0, 1.0, 0.0]),
        v([0.0, s, Z], [0.0, b], [0.0, 1.0, 0.0]),
        v([s, s, Z], [1.0, b], [0.0, 1.0, 0.0]),
        v([s, s, -Z], [1.0, 0.0], [0.0, 1.0, 0.0]),
    ]));
    out.extend(quad([
        v([0.0, 0.0, -Z], [0.0, 0.0], [0.0, -1.0, 0.0]),
        v([s, 0.0, -Z], [1.0, 0.0], [0.0, -1.0, 0.0]),
        v([s, 0.0, Z], [1.0, b], [0.0, -1.0, 0.0]),
        v([0.0, 0.0, Z], [0.0, b], [0.0, -1.0, 0.0]),
    ]));
    // Sides: strip of 1/16 width.
    out.extend(quad([
        v([0.0, 0.0, -Z], [0.0, 1.0], [-1.0, 0.0, 0.0]),
        v([0.0, 0.0, Z], [b, 1.0], [-1.0, 0.0, 0.0]),
        v([0.0, s, Z], [b, 0.0], [-1.0, 0.0, 0.0]),
        v([0.0, s, -Z], [0.0, 0.0], [-1.0, 0.0, 0.0]),
    ]));
    out.extend(quad([
        v([s, 0.0, -Z], [0.0, 1.0], [1.0, 0.0, 0.0]),
        v([s, s, -Z], [0.0, 0.0], [1.0, 0.0, 0.0]),
        v([s, s, Z], [b, 0.0], [1.0, 0.0, 0.0]),
        v([s, 0.0, Z], [b, 1.0], [1.0, 0.0, 0.0]),
    ]));
    out
}

/// Layer of an armor piece: model, texture rectangle, default dye colour
/// (dyeable only).
#[derive(Clone, Copy)]
struct ArmorLayer {
    layer: usize,
    rect: [f32; 4],
    dye: Option<u32>,
}

/// Instance: layer of an entity in the frame.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    /// First part matrix of the instance.
    base: u32,
    /// Texture rectangle in the atlas.
    rect: [f32; 4],
    /// RGBA colour.
    color: u32,
    /// Light: block × 16 | sky × 16 << 8.
    light: u32,
    /// Overlay: u | v << 16.
    overlay: u32,
}

/// Matrix of a part (full pose) and normal matrix (columns of 3 + 1 padding).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PartMat {
    m: [[f32; 4]; 4],
    n: [[f32; 4]; 3],
}

impl PartMat {
    /// Normal matrix: cofactors of the 3 × 3 part (inverse transpose up to a factor;
    /// the shader normalises), determinant sign kept (mirrored parts).
    fn new(m: &Mat4) -> Self {
        let (x, y, z) = (m.x_axis.truncate(), m.y_axis.truncate(), m.z_axis.truncate());
        let (cx, cy, cz) = (y.cross(z), z.cross(x), x.cross(y));
        let sign = if x.dot(cx) < 0.0 { -1.0 } else { 1.0 };
        let c = |v: Vec3| [v.x * sign, v.y * sign, v.z * sign, 0.0];
        PartMat { m: m.to_cols_array_2d(), n: [c(cx), c(cy), c(cz)] }
    }

    fn zeroed() -> Self {
        bytemuck::Zeroable::zeroed()
    }
}

/// Atlas of the entity textures (all those of `textures/entity/`), by short name
/// (`entity/zombie/zombie`): rectangle in fractions.
pub struct EntityTextures {
    /// Textures (name → compressed PNG or image), loaded into the atlas on demand.
    pub sources: HashMap<String, crate::entity_atlas::Source>,
    /// Vanilla variants: (registry, entry) → definition.
    pub variants: HashMap<(String, String), VariantDef>,
    /// Equipment: asset → layer type → layers (texture, default colour if dyeable).
    pub equipment: Equipment,
    /// Hats of the villager type and profession textures (`.mcmeta`): texture → 0 none,
    /// 1 partial, 2 full.
    pub villager_hats: HashMap<String, u8>,
    /// Block models per state (minecarts, TNT, falling blocks, displays) and named block
    /// models outside states (frames).
    blocks: Option<Arc<rc_assets::BlockModels>>,
    named_models: HashMap<String, Vec<objects::ModelQuad>>,
    /// Default colour of the grass palette ("position-less" tint of blocks drawn
    /// as entities).
    grass_default: u32,
    /// Resources, read by the atlas on demand.
    assets: Arc<rc_assets::source::Assets>,
    /// Geometry of the special item models (heads, banners, shields, chests, Shulker).
    pub special: crate::item_special::SpecialAssets,
    /// Animated textures copied into the atlas (flames, conduit wind, `#item/<i>`,
    /// `#block/<i>`) and their playback.
    anims: Vec<(String, crate::sprite_anim::SpriteAnim)>,
}

/// Animated atlas textures read at startup: (name in the atlas, id). The
/// flames of an entity on fire reuse the two textures of the fire block.
const ANIMATED: [(&str, &str); 4] = [
    ("#fire_0", "minecraft:block/fire_0"),
    ("#fire_1", "minecraft:block/fire_1"),
    ("entity/conduit/wind", "minecraft:entity/conduit/wind"),
    ("entity/conduit/wind_vertical", "minecraft:entity/conduit/wind_vertical"),
];

/// Files of [`ANIMATED`] (resource cache): images, meta.
pub fn animated_paths() -> impl Iterator<Item = (String, String)> {
    ANIMATED.iter().map(|(_, id)| {
        let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
        let png = format!("assets/{ns}/textures/{path}.png");
        let meta = format!("{png}.mcmeta");
        (png, meta)
    })
}

/// Animal variant (`data/minecraft/<animal>_variant/*.json`): adult and baby textures
/// (paths `entity/...`), model (`normal`, `cold`, `warm`).
#[derive(Debug, Clone, Default)]
pub struct VariantDef {
    pub asset: String,
    pub baby_asset: String,
    pub model: String,
    /// Wolf: tamed and angry textures (adult, baby).
    pub tame: (String, String),
    pub angry: (String, String),
    /// Paintings: size in blocks.
    pub width: u8,
    pub height: u8,
}

/// Key of the starting variant of an entity of this registry (pale wolf, black cat, otherwise temperate).
fn default_variant(registry: &str) -> &'static str {
    match registry {
        "minecraft:wolf_variant" => "minecraft:pale",
        "minecraft:cat_variant" => "minecraft:black",
        _ => "minecraft:temperate",
    }
}

fn asset_path(id: &str) -> String {
    id.strip_prefix("minecraft:").unwrap_or(id).to_owned()
}

impl VariantDef {
    fn from_json(v: &serde_json::Value) -> Self {
        // Wolf: "wild" textures by default.
        let asset = v["asset_id"].as_str().or_else(|| v["assets"]["wild"].as_str()).unwrap_or_default();
        let baby = v["baby_asset_id"].as_str().or_else(|| v["baby_assets"]["wild"].as_str()).unwrap_or(asset);
        let pair = |k: &str| {
            let a = v["assets"][k].as_str().map_or_else(|| asset_path(asset), asset_path);
            let b = v["baby_assets"][k].as_str().map_or_else(|| a.clone(), asset_path);
            (a, b)
        };
        VariantDef {
            asset: asset_path(asset),
            baby_asset: asset_path(baby),
            model: v["model"].as_str().unwrap_or("normal").to_owned(),
            tame: pair("tame"),
            angry: pair("angry"),
            width: v["width"].as_u64().unwrap_or(1).min(16) as u8,
            height: v["height"].as_u64().unwrap_or(1).min(16) as u8,
        }
    }

    fn from_nbt(n: &rc_proto::nbt::Nbt) -> Self {
        let s = |k: &str| n.get(k).and_then(rc_proto::nbt::Nbt::as_str).map(asset_path);
        let asset = s("asset_id").or_else(|| n.get("assets").and_then(|a| a.get("wild")?.as_str()).map(asset_path));
        let asset = asset.unwrap_or_default();
        let pair = |k: &str| {
            let get = |g: &str| n.get(g).and_then(|a| a.get(k)?.as_str()).map(asset_path);
            let a = get("assets").unwrap_or_else(|| asset.clone());
            let b = get("baby_assets").unwrap_or_else(|| a.clone());
            (a, b)
        };
        VariantDef {
            tame: pair("tame"),
            angry: pair("angry"),
            baby_asset: s("baby_asset_id")
                .or_else(|| n.get("baby_assets").and_then(|a| a.get("wild")?.as_str()).map(asset_path))
                .unwrap_or_else(|| asset.clone()),
            asset,
            model: n.get("model").and_then(rc_proto::nbt::Nbt::as_str).unwrap_or("normal").to_owned(),
            width: n.get("width").and_then(rc_proto::nbt::Nbt::as_i64).unwrap_or(1).clamp(1, 16) as u8,
            height: n.get("height").and_then(rc_proto::nbt::Nbt::as_i64).unwrap_or(1).clamp(1, 16) as u8,
        }
    }
}

/// Vanilla variants read from the jar (content omitted by the server for a known pack).
fn load_variants(assets: &rc_assets::source::Assets) -> HashMap<(String, String), VariantDef> {
    let mut out = HashMap::new();
    for animal in ["cow", "pig", "chicken", "cat", "frog", "wolf", "painting", "zombie_nautilus"] {
        let dir = format!("data/minecraft/{animal}_variant/");
        for f in assets.list(&dir, ".json") {
            let Some(name) = f.strip_prefix(&dir).and_then(|n| n.strip_suffix(".json")) else { continue };
            let Some(json) = assets.read(&f).and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) else {
                continue;
            };
            out.insert(
                (format!("minecraft:{animal}_variant"), format!("minecraft:{name}")),
                VariantDef::from_json(&json),
            );
        }
    }
    out
}

/// Equipment: asset → layer type → layers (texture, default colour if dyeable).
pub type Equipment = HashMap<String, HashMap<String, Vec<(String, Option<u32>)>>>;

/// Definitions `assets/<ns>/equipment/*.json` (`entities.md` part 3 § 3.5).
fn load_equipment(assets: &rc_assets::source::Assets) -> Equipment {
    let mut out = HashMap::new();
    for f in assets.list("assets/minecraft/equipment/", ".json") {
        let Some(name) = f.strip_prefix("assets/minecraft/equipment/").and_then(|n| n.strip_suffix(".json")) else {
            continue;
        };
        let Some(json) = assets.read(&f).and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) else {
            continue;
        };
        let mut types = HashMap::new();
        for (ty, list) in json["layers"].as_object().into_iter().flatten() {
            let layers = list
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|l| {
                    let tex = l["texture"].as_str()?;
                    let dye = l.get("dyeable").map(|d| d["color_when_undyed"].as_i64().map_or(0, |c| c as u32));
                    Some((tex.strip_prefix("minecraft:").unwrap_or(tex).to_owned(), dye))
                })
                .collect();
            // Layer that takes the player's texture (elytra: cape): type marked "@player".
            if list.as_array().into_iter().flatten().any(|l| l["use_player_texture"].as_bool() == Some(true)) {
                types.insert(format!("{ty}@player"), Vec::new());
            }
            types.insert(ty.clone(), layers);
        }
        out.insert(format!("minecraft:{name}"), types);
    }
    out
}

/// Hats declared by the `.mcmeta` of the type and profession textures (villagers and
/// zombie villagers): `{"villager": {"hat": "none" | "partial" | "full"}}`.
fn load_villager_hats(assets: &rc_assets::source::Assets) -> HashMap<String, u8> {
    let mut out = HashMap::new();
    for dir in ["villager", "zombie_villager"] {
        for kind in ["type", "profession"] {
            let base = format!("assets/minecraft/textures/entity/{dir}/{kind}/");
            for f in assets.list(&base, ".png.mcmeta") {
                let Some(name) =
                    f.strip_prefix("assets/minecraft/textures/").and_then(|n| n.strip_suffix(".png.mcmeta"))
                else {
                    continue;
                };
                let Some(json) = assets.read(&f).and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                else {
                    continue;
                };
                let hat = match json["villager"]["hat"].as_str() {
                    Some("partial") => 1,
                    Some("full") => 2,
                    _ => 0,
                };
                out.insert(name.to_owned(), hat);
            }
        }
    }
    out
}

impl EntityTextures {
    /// Lists the entity textures (paths only: read and decoded on first use).
    pub fn load(assets: &Arc<rc_assets::source::Assets>, item_sprites: &[Image]) -> Self {
        use crate::entity_atlas::Source;
        let mut sources: HashMap<String, Source> = HashMap::new();
        // Armor trims: registries and palettes; base textures (`trims/`) read on
        // demand to be recoloured (`item_regs.rs`).
        crate::item_regs::load(assets);
        for dir in ["entity/", "painting/", "map/", "trims/"] {
            for f in assets.list(&format!("assets/minecraft/textures/{dir}"), ".png") {
                if let Some(name) = f.strip_prefix("assets/minecraft/textures/").and_then(|n| n.strip_suffix(".png")) {
                    sources.insert(name.to_owned(), Source::Asset(f.clone()));
                }
            }
        }
        sources.insert("misc/shadow".to_owned(), Source::Asset("assets/minecraft/textures/misc/shadow.png".into()));
        sources.insert(
            "#glint".to_owned(),
            Source::Asset("assets/minecraft/textures/misc/enchanted_glint_item.png".into()),
        );
        sources.insert(
            "#glint_armor".to_owned(),
            Source::Asset("assets/minecraft/textures/misc/enchanted_glint_armor.png".into()),
        );
        // Animated textures read at startup (flames of an entity on fire, conduit wind): an
        // image at the size declared by the `.png.mcmeta` (not the first square of the strip), then
        // rewritten at each step of their playback (`tick_textures`).
        let mut anims = Vec::new();
        for &(name, id) in &ANIMATED {
            let Some(l) = rc_assets::texture::load_animated(assets, id) else { continue };
            let first = l.animation.as_ref().map_or(0, |a| a.frames[0].0 as usize);
            let Some(img) = l.frames.get(first).cloned() else { continue };
            sources.insert(name.to_owned(), Source::Image(img));
            if let Some(a) = &l.animation
                && let Some(s) = crate::sprite_anim::SpriteAnim::new(l.frames.into_iter().map(|f| vec![f]).collect(), a)
            {
                anims.push((name.to_owned(), s));
            }
        }
        // Sprites of the item models (ground items, held items), named `#item/<index>`.
        sources
            .extend(item_sprites.iter().enumerate().map(|(i, img)| (format!("#item/{i}"), Source::Image(img.clone()))));
        let named_models = objects::load_named_models(assets, &mut sources);
        // Default grass colour (palette at temperature 0.5, humidity 1), opaque ARGB.
        let grass = rc_assets::texture::load(assets, "minecraft:colormap/grass");
        let grass_default = 0xff00_0000 | rc_assets::texture::colormap(grass.as_ref(), 0.5, 1.0, 0xff_00ff);
        EntityTextures {
            grass_default,
            sources,
            variants: load_variants(assets),
            equipment: load_equipment(assets),
            villager_hats: load_villager_hats(assets),
            blocks: None,
            named_models,
            assets: assets.clone(),
            special: Default::default(),
            anims,
        }
    }

    /// Animations of the item sprites (`#item/<i>`: ground, held, in a frame items), in
    /// sprite order: frames of the strip and playback.
    pub fn with_item_animations(
        mut self,
        animations: &[Option<rc_assets::texture::Animation>],
        frames: &[Vec<Image>],
    ) -> Self {
        for (i, (a, f)) in animations.iter().zip(frames).enumerate() {
            let Some(a) = a else { continue };
            if let Some(s) = crate::sprite_anim::SpriteAnim::new(f.iter().map(|img| vec![img.clone()]).collect(), a) {
                self.anims.push((format!("#item/{i}"), s));
            }
        }
        self
    }

    /// Paths of the textures read on demand or at startup (put into the resource cache).
    pub fn asset_paths(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .sources
            .values()
            .filter_map(|s| match s {
                crate::entity_atlas::Source::Asset(p) => Some(p.clone()),
                _ => None,
            })
            .chain(animated_paths().map(|p| p.0))
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Baked block models and first frames of their textures (named `#block/<index>`),
    /// to draw a block in an entity.
    pub fn with_blocks(mut self, models: Arc<rc_assets::BlockModels>, textures: &[Vec<Image>]) -> Self {
        use crate::entity_atlas::Source;
        // Named textures: re-read from the resources on first use (first frame);
        // animation frames (unnamed): kept decoded.
        for (i, levels) in textures.iter().enumerate() {
            let source = match models.texture_names.get(i) {
                Some(name) => {
                    let (ns, path) = name.split_once(':').unwrap_or(("minecraft", name));
                    Source::Asset(format!("assets/{ns}/textures/{path}.png"))
                }
                None => match levels.first() {
                    Some(img) => Source::Image(img.clone()),
                    None => continue,
                },
            };
            self.sources.insert(format!("#block/{i}"), source);
        }
        // Animated textures of blocks drawn in an entity (falling blocks, minecarts,
        // displays, block held by the Enderman): same steps as the block atlas, frames
        // taken from its layers (all sizes, the atlas keeps the original size).
        for a in &models.animations {
            let mut layers: Vec<u32> = Vec::new();
            let frames = a
                .steps
                .iter()
                .map(|&(layer, time)| {
                    let k = layers.iter().position(|&l| l == layer).unwrap_or_else(|| {
                        layers.push(layer);
                        layers.len() - 1
                    });
                    (k as u32, time)
                })
                .collect();
            let images = layers.iter().map(|&l| textures.get(l as usize).cloned().unwrap_or_default()).collect();
            let anim = rc_assets::texture::Animation { frames, interpolate: a.interpolate };
            if let Some(s) = crate::sprite_anim::SpriteAnim::new(images, &anim) {
                self.anims.push((format!("#block/{}", a.texture), s));
            }
        }
        self.blocks = Some(models);
        self
    }
}

/// Baked quad of a part: positions in pixels of the part's frame, UV normalised to the
/// layer, normal.
#[derive(Clone, Copy)]
struct Quad {
    pos: [[f32; 3]; 4],
    uv: [[f32; 2]; 4],
    normal: [f32; 3],
}

struct BakedPart {
    parent: u32,
    pose: [f32; 9],
    quads: Vec<Quad>,
}

/// Known animated parts: their index in each layer is computed at baking time (the
/// animations designate them without hashing a string at every frame).
const KNOWN: usize = 36;

fn known_id(name: &str) -> Option<usize> {
    Some(match name {
        "head" => 0,
        "body" => 1,
        "right_arm" => 2,
        "left_arm" => 3,
        "right_leg" => 4,
        "left_leg" => 5,
        "right_hind_leg" => 6,
        "left_hind_leg" => 7,
        "right_front_leg" => 8,
        "left_front_leg" => 9,
        "right_wing" => 10,
        "left_wing" => 11,
        "right_middle_hind_leg" => 12,
        "left_middle_hind_leg" => 13,
        "right_middle_front_leg" => 14,
        "left_middle_front_leg" => 15,
        "jacket" => 16,
        "left_sleeve" => 17,
        "right_sleeve" => 18,
        "left_pants" => 19,
        "right_pants" => 20,
        "hat" => 21,
        "left_ear" => 22,
        "right_ear" => 23,
        "arms" => 24,
        "mushrooms" => 25,
        "neck" => 26,
        "mouth" => 27,
        "base_plate" => 28,
        "right_body_stick" => 29,
        "left_body_stick" => 30,
        "shoulder_stick" => 31,
        "upper_jaw" => 32,
        "lower_jaw" => 33,
        "base" => 34,
        "root" => 35,
        _ => return None,
    })
}

/// Baked layer: parts in drawing order, index of the known parts.
pub struct BakedLayer {
    parts: Vec<BakedPart>,
    known: [u16; KNOWN],
    /// Name of each part (animations by name outside the table of known parts).
    names: Vec<String>,
}

/// Cube face: direction bit, vertices, UV rectangle, normal.
type FaceDef = (u8, [[f32; 3]; 4], [f32; 4], [f32; 3]);

/// Faces of a cube (`entities.md` part 2 § 1.4): order bottom, top, west, north, east, south.
fn bake_cube(c: &rc_data::EntityCube, tex: [u16; 2], out: &mut Vec<Quad>) {
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
    // Sums from left to right (exact to the last bit).
    let u0 = u;
    let u1 = u + d;
    let u2 = u + d + w;
    let u22 = u + d + w + w;
    let u3 = u + d + w + d;
    let u4 = u + d + w + d + w;
    let (v0, v1, v2) = (v, v + d, v + d + h);
    let tw = f32::from(tex[0]) * c.tex_scale[0];
    let th = f32::from(tex[1]) * c.tex_scale[1];
    // (direction bit (order bottom, top, north, south, west, east), vertices, UV rectangle, normal).
    let faces: [FaceDef; 6] = [
        (0, [l[1], l[0], t[0], t[1]], [u1, v0, u2, v1], [0.0, -1.0, 0.0]),
        (1, [t[2], t[3], l[3], l[2]], [u2, v1, u22, v0], [0.0, 1.0, 0.0]),
        (4, [t[0], l[0], l[3], t[3]], [u0, v1, u1, v2], [-1.0, 0.0, 0.0]),
        (2, [t[1], t[0], t[3], t[2]], [u1, v1, u2, v2], [0.0, 0.0, -1.0]),
        (5, [l[1], t[1], t[2], l[2]], [u2, v1, u3, v2], [1.0, 0.0, 0.0]),
        (3, [l[0], l[1], l[2], l[3]], [u3, v1, u4, v2], [0.0, 0.0, 1.0]),
    ];
    for (bit, pos, r, n) in faces {
        if c.faces & (1 << bit) == 0 {
            continue;
        }
        let mut uv = [[r[2] / tw, r[1] / th], [r[0] / tw, r[1] / th], [r[0] / tw, r[3] / th], [r[2] / tw, r[3] / th]];
        let mut pos = pos;
        let mut normal = n;
        if c.mirror {
            pos.reverse();
            uv.reverse();
            if n[0] != 0.0 {
                normal[0] = -n[0];
            }
        }
        out.push(Quad { pos, uv, normal });
    }
}

fn bake_layer(l: &EntityLayer) -> BakedLayer {
    let mut known = [u16::MAX; KNOWN];
    let parts = l
        .parts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if let Some(k) = known_id(&p.name)
                && known[k] == u16::MAX
            {
                known[k] = i as u16;
            }
            let mut quads = Vec::new();
            for c in &p.cubes {
                bake_cube(c, l.texture, &mut quads);
            }
            BakedPart { parent: p.parent, pose: p.pose, quads }
        })
        .collect();
    BakedLayer { parts, known, names: l.parts.iter().map(|p| p.name.clone()).collect() }
}

/// Animated pose of a layer: pose per part and visibility.
pub struct Pose<'a> {
    layer: &'a BakedLayer,
    pub parts: Vec<[f32; 9]>,
    pub visible: Vec<bool>,
}

impl<'a> Pose<'a> {
    /// Definition pose of the layer, in reused buffers.
    fn reset(layer: &'a BakedLayer, mut parts: Vec<[f32; 9]>, mut visible: Vec<bool>) -> Self {
        parts.clear();
        parts.extend(layer.parts.iter().map(|p| p.pose));
        visible.clear();
        visible.resize(layer.parts.len(), true);
        Pose { layer, parts, visible }
    }

    /// Part by name: table of known parts, otherwise search in the layer's names.
    fn index(&self, name: &str) -> Option<usize> {
        match known_id(name) {
            Some(k) => {
                let i = self.layer.known[k];
                (i != u16::MAX).then_some(usize::from(i))
            }
            None => self.layer.names.iter().position(|n| n == name),
        }
    }

    fn get(&mut self, name: &str) -> Option<&mut [f32; 9]> {
        let i = self.index(name)?;
        Some(&mut self.parts[i])
    }

    fn set(&mut self, name: &str, f: impl FnOnce(&mut [f32; 9])) {
        if let Some(p) = self.get(name) {
            f(p);
        }
    }

    fn has(&self, name: &str) -> bool {
        self.index(name).is_some()
    }

    fn hide(&mut self, name: &str) {
        if let Some(i) = self.index(name) {
            self.visible[i] = false;
        }
    }
}

/// Local matrix of a part: T(x/16) · Rz · Ry · Rx · S.
fn part_matrix(p: &[f32; 9]) -> Mat4 {
    let t = Vec3::new(p[0] / 16.0, p[1] / 16.0, p[2] / 16.0);
    let s = Vec3::new(p[6], p[7], p[8]);
    // T · Rz · Ry · Rx · S, composed directly (zero rotation: most frequent case).
    if p[3] == 0.0 && p[4] == 0.0 && p[5] == 0.0 {
        return Mat4::from_scale_rotation_translation(s, glam::Quat::IDENTITY, t);
    }
    // Rz · Ry · Rx as quaternions, zero angles skipped (frequent case: one or two axes).
    let axis = |angle: f32, f: fn(f32, f32) -> glam::Quat| {
        if angle == 0.0 {
            glam::Quat::IDENTITY
        } else {
            let (s, c) = sin_cos(angle * 0.5);
            f(s, c)
        }
    };
    let q = axis(p[5], |s, c| glam::Quat::from_xyzw(0.0, 0.0, s, c))
        * axis(p[4], |s, c| glam::Quat::from_xyzw(0.0, s, 0.0, c))
        * axis(p[3], |s, c| glam::Quat::from_xyzw(s, 0.0, 0.0, c));
    Mat4::from_scale_rotation_translation(s, q, t)
}

/// Sine and cosine (rendering only): reduction to a quarter turn, minimax polynomials on
/// [−π/4, π/4]; error < 1e-6 on [−20, 20] (a few ulp of f32), invisible when rendered.
fn sin_cos(x: f32) -> (f32, f32) {
    let q = (x * std::f32::consts::FRAC_2_PI).round();
    // Two-step reduction (π/2 = high + low) to keep the precision.
    let r = (x - q * 1.570_796_4) + q * 4.371_139e-8;
    let r2 = r * r;
    let s = r * (1.0 + r2 * (-0.166_666_67 + r2 * (0.008_333_294 + r2 * -0.000_198_074_2)));
    let c = 1.0 + r2 * (-0.5 + r2 * (0.041_666_42 + r2 * (-0.001_388_731 + r2 * 2.443_315_7e-5)));
    match (q as i32) & 3 {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    }
}

/// Common animation state (`entities.md` part 3 § 1.1).
struct AnimState {
    walk_pos: f32,
    walk_speed: f32,
    age: f32,
    /// Head relative to the body and pitch, in radians.
    yaw: f32,
    pitch: f32,
    swing: f32,
    swing_right: bool,
    /// A swing is in progress (the existence of the swing, not the animation progress).
    swing_on: bool,
    swing_kind: i32,
    crouching: bool,
    baby: bool,
    /// Passenger of a vehicle: seated posture, zero walk.
    riding: bool,
    /// Pose of each arm (right, left) of a player, codes of `pose_arms`; 0 = none.
    arm_pose: [u8; 2],
    /// Arm posed first (use in progress, otherwise off hand except two-handed pose).
    arm_first_right: bool,
    /// A crossbow is held in one of the two hands (pillager pose).
    crossbow_held: bool,
    /// Swim progress (0..1).
    swim: f32,
    /// Glide (elytra).
    fall_flying: bool,
    /// Divisor of the limbs while gliding (speed² / 0.2, cubed, at least 1).
    speed: f32,
    /// Use of an item in progress, and ticks elapsed since its start.
    using: bool,
    use_ticks: f32,
    /// Charge duration of the crossbow in use (Quick Charge included), in ticks.
    crossbow_charge: f32,
    /// Spear held per arm (right, left): durations of its kinetic weapon.
    spear: [Option<crate::hand::Kinetic>; 2],
    /// The current use is of the right arm.
    use_right: bool,
    /// Box in contact with water, noted at the tick (`water_track`): frog, turtle, equines.
    in_water: bool,
    /// Arm pose of a piglin outside dancing (`piglin_pose::*`), read from its equipment.
    piglin: u8,
}

/// Arm poses of a piglin, by priority: admiring (liked item in the off hand), melee weapon
/// brandished (aggressive, tool in hand), crossbow charging, loaded crossbow held.
mod piglin_pose {
    pub const DEFAULT: u8 = 0;
    pub const ADMIRING: u8 = 1;
    pub const MELEE: u8 = 2;
    pub const CHARGE: u8 = 3;
    pub const HOLD: u8 = 4;
}

/// Arm poses: empty, item, blocking, bow, trident, crossbow
/// (loading, held), spyglass, horn, brush.
mod arm {
    pub const EMPTY: u8 = 1;
    pub const ITEM: u8 = 2;
    pub const BLOCK: u8 = 3;
    pub const BOW: u8 = 4;
    pub const TRIDENT: u8 = 5;
    pub const CHARGE: u8 = 6;
    pub const HOLD: u8 = 7;
    pub const SPYGLASS: u8 = 8;
    pub const HORN: u8 = 9;
    pub const BRUSH: u8 = 10;
    pub const SPEAR: u8 = 11;
    pub const fn two_handed(p: u8) -> bool {
        matches!(p, BOW | CHARGE | HOLD)
    }
    /// The pose also sets the other arm (the second arm's pose is then skipped).
    pub const fn affects_offhand(p: u8) -> bool {
        matches!(p, BOW | TRIDENT | CHARGE | HOLD)
    }
}

/// Pose of a player's arm (right arm pose/left arm pose); head relative to the body in `s`.
fn pose_one_arm(p: &mut Pose<'_>, s: &AnimState, right: bool) {
    let (me, other) = if right { ("right_arm", "left_arm") } else { ("left_arm", "right_arm") };
    let sign = if right { 1.0 } else { -1.0 };
    // Angles of the head already posed: raised by 45° while gliding.
    let (hx, hy) = (head_pitch(s), s.yaw);
    match s.arm_pose[usize::from(!right)] {
        arm::EMPTY => p.set(me, |a| a[4] = 0.0),
        arm::ITEM => p.set(me, |a| {
            a[3] = a[3] * 0.5 - PI / 10.0;
            a[4] = 0.0;
        }),
        arm::BLOCK => p.set(me, |a| {
            a[3] = a[3] * 0.5 - 0.942_477_9 + hx.clamp(-1.396_263_4, 0.436_332_32);
            a[4] = -sign * 30.0 * PI / 180.0 + hy.clamp(-PI / 6.0, PI / 6.0);
        }),
        arm::BOW => {
            // Aiming arm: offset of ±0.4 on the opposite side.
            let (mine, off) = if right { (-0.1 + hy, 0.1 + hy + 0.4) } else { (-0.1 + hy - 0.4, 0.1 + hy) };
            p.set("right_arm", |a| {
                a[4] = mine;
                a[3] = -PI / 2.0 + hx;
            });
            p.set("left_arm", |a| {
                a[4] = off;
                a[3] = -PI / 2.0 + hx;
            });
        }
        arm::TRIDENT => p.set(me, |a| {
            a[3] = a[3] * 0.5 - PI;
            a[4] = 0.0;
        }),
        arm::CHARGE => {
            // The arm that draws the string follows the charge (25 ticks without Quick Charge).
            let k = s.use_ticks.clamp(0.0, s.crossbow_charge) / s.crossbow_charge;
            p.set(me, |a| {
                a[4] = -sign * 0.8;
                a[3] = -0.970_796_35;
            });
            p.set(other, |a| {
                a[4] = sign * (0.4 + k * (0.85 - 0.4));
                a[3] = -0.970_796_35 + k * (-PI / 2.0 + 0.970_796_35);
            });
        }
        arm::HOLD => {
            p.set(me, |a| {
                a[4] = -sign * 0.3 + hy;
                a[3] = -PI / 2.0 + hx + 0.1;
            });
            p.set(other, |a| {
                a[4] = sign * 0.6 + hy;
                a[3] = -1.5 + hx;
            });
        }
        arm::SPYGLASS => p.set(me, |a| {
            a[3] = (hx - 1.919_862_2 - if s.crouching { PI / 12.0 } else { 0.0 }).clamp(-2.4, 3.3);
            a[4] = hy - sign * PI / 12.0;
        }),
        arm::HORN => p.set(me, |a| {
            a[3] = hx.clamp(-1.2, 1.2) - 1.483_529_8;
            a[4] = hy - sign * PI / 6.0;
        }),
        arm::BRUSH => p.set(me, |a| {
            a[3] = a[3] * 0.5 - PI / 5.0;
            a[4] = 0.0;
        }),
        arm::SPEAR => {
            let k = s.spear[usize::from(!right)];
            let using = k.filter(|_| !s.using || s.use_right == right).map(|k| (s.use_ticks, k));
            let flying = s.fall_flying || s.swim > 0.0;
            p.set(me, |a| {
                let [x, y, z] = crate::spear_anim::hand_pose([a[3], a[4], a[5]], (hx, hy), right, flying, using);
                (a[3], a[4], a[5]) = (x, y, z);
            });
        }
        _ => {}
    }
}

/// Poses of a player's two arms, in the game's order, before the attack.
fn pose_arms(p: &mut Pose<'_>, s: &AnimState) {
    let first = s.arm_first_right;
    pose_one_arm(p, s, first);
    if !arm::affects_offhand(s.arm_pose[usize::from(!first)]) {
        pose_one_arm(p, s, !first);
    }
}

const fn deg(d: f32) -> f32 {
    d * PI / 180.0
}

/// Pitch of the humanoid head: raised by 45° forwards while gliding.
fn head_pitch(s: &AnimState) -> f32 {
    if s.fall_flying { -PI / 4.0 } else { s.pitch }
}

fn humanoid(p: &mut Pose<'_>, s: &AnimState) {
    let (pp, v) = (s.walk_pos, s.walk_speed);
    p.set("head", |h| {
        h[3] = head_pitch(s);
        h[4] = s.yaw;
    });
    let q = pp * 0.6662;
    let sp = s.speed;
    p.set("right_arm", |a| a[3] = (q + PI).cos() * 2.0 * v * 0.5 / sp);
    p.set("left_arm", |a| a[3] = q.cos() * 2.0 * v * 0.5 / sp);
    p.set("right_leg", |l| {
        l[3] = q.cos() * 1.4 * v / sp;
        l[4] = 0.005;
        l[5] = 0.005;
    });
    p.set("left_leg", |l| {
        l[3] = (q + PI).cos() * 1.4 * v / sp;
        l[4] = -0.005;
        l[5] = -0.005;
    });
    // Seated passenger (`entities.md` § 3).
    if s.riding {
        p.set("right_arm", |a| a[3] += -PI / 5.0);
        p.set("left_arm", |a| a[3] += -PI / 5.0);
        p.set("right_leg", |l| {
            l[3] = -1.413_716_7;
            l[4] = PI / 10.0;
            l[5] = 0.078_539_82;
        });
        p.set("left_leg", |l| {
            l[3] = -1.413_716_7;
            l[4] = -PI / 10.0;
            l[5] = -0.078_539_82;
        });
    }
    if s.arm_pose != [0, 0] {
        pose_arms(p, s);
    }
    // `whack` attack (§ 2.1.2).
    if s.swing > 0.0 && s.swing_on {
        let mut body = (s.swing.sqrt() * 2.0 * PI).sin() * 0.2;
        if !s.swing_right {
            body = -body;
        }
        let age_scale = if s.baby { 0.5 } else { 1.0 };
        p.set("body", |b| b[4] = body);
        p.set("right_arm", |a| {
            a[2] = body.sin() * 5.0 * age_scale;
            a[0] = -body.cos() * 5.0 * age_scale;
            a[4] += body;
        });
        p.set("left_arm", |a| {
            a[2] = -body.sin() * 5.0 * age_scale;
            a[0] = body.cos() * 5.0 * age_scale;
            a[4] += body;
            a[3] += body;
        });
        if s.swing_kind == 1 {
            let out = 1.0 - (1.0 - s.swing).powi(4);
            let a = (out * PI).sin();
            let head_x = p.get("head").map_or(0.0, |h| h[3]);
            let b = (s.swing * PI).sin() * -(head_x - 0.7) * 0.75;
            p.set(if s.swing_right { "right_arm" } else { "left_arm" }, |arm| {
                arm[3] -= a * 1.2 + b;
                arm[4] += body * 2.0;
                arm[5] += (s.swing * PI).sin() * -0.4;
            });
        } else if s.swing_kind == 2 {
            // Lunge: body rotation removed from the arms, then thrust of the striking arm.
            p.set("right_arm", |a| a[4] -= body);
            p.set("left_arm", |a| {
                a[4] -= body;
                a[3] -= body;
            });
            let push = crate::spear_anim::attack_arm(s.swing);
            p.set(if s.swing_right { "right_arm" } else { "left_arm" }, |a| a[3] += push);
        }
    }
    idle_and_crouch(p, s);
    if s.swim > 0.0 {
        swim_arms(p, s);
    }
}

/// Angular interpolation along the shortest path (radians).
fn rot_lerp_rad(t: f32, a: f32, b: f32) -> f32 {
    let mut d = (b - a) % (2.0 * PI);
    if d >= PI {
        d -= 2.0 * PI;
    }
    if d < -PI {
        d += 2.0 * PI;
    }
    a + t * d
}

/// Breaststroke (`entities.md`, humanoid swimming): cycle of 26 over the walk position, arms
/// in an arc then brought back, legs kicking at a third of the rhythm; head tilted by 45° outside gliding.
/// The striking arm does not swim; no breaststroke during a use.
fn swim_arms(p: &mut Pose<'_>, s: &AnimState) {
    let w = s.swim;
    if !s.fall_flying {
        p.set("head", |h| h[3] = rot_lerp_rad(w, h[3], -PI / 4.0));
    }
    let pos = s.walk_pos % 26.0;
    let swinging = s.swing_on;
    // Arm holding a spear or striking: no breaststroke.
    let rw = if (swinging && s.swing_right) || s.arm_pose[0] == arm::SPEAR { 0.0 } else { w };
    let lw = if (swinging && !s.swing_right) || s.arm_pose[1] == arm::SPEAR { 0.0 } else { w };
    let quad = |x: f32| -65.0 * x + x * x;
    if !s.using {
        // Targeted (x, y, z): left arm (interpolated along the shortest path), right arm (linear).
        let (lt, rt) = if pos < 14.0 {
            let k = 1.870_796_4 * quad(pos) / quad(14.0);
            ([0.0, PI, PI + k], [0.0, PI, PI - k])
        } else if pos < 22.0 {
            let t = (pos - 14.0) / 8.0;
            ([PI / 2.0 * t, PI, 5.012_389 - 1.870_796_4 * t], [PI / 2.0 * t, PI, 1.270_796_3 + 1.870_796_4 * t])
        } else {
            let t = (pos - 22.0) / 4.0;
            ([PI / 2.0 - PI / 2.0 * t, PI, PI], [PI / 2.0 - PI / 2.0 * t, PI, PI])
        };
        p.set("left_arm", |a| {
            a[3] = rot_lerp_rad(lw, a[3], lt[0]);
            a[4] = rot_lerp_rad(lw, a[4], lt[1]);
            a[5] = rot_lerp_rad(lw, a[5], lt[2]);
        });
        p.set("right_arm", |a| {
            a[3] += rw * (rt[0] - a[3]);
            a[4] += rw * (rt[1] - a[4]);
            a[5] += rw * (rt[2] - a[5]);
        });
    }
    p.set("left_leg", |l| l[3] += w * (0.3 * (s.walk_pos / 3.0 + PI).cos() - l[3]));
    p.set("right_leg", |l| l[3] += w * (0.3 * (s.walk_pos / 3.0).cos() - l[3]));
}

/// Drowned swim: arms stretched forward that wave, alternating legs, head straight.
fn drowned_swim(p: &mut Pose<'_>, s: &AnimState) {
    let w = s.swim;
    let o = (0.1 * s.age).sin();
    p.set("right_arm", |a| {
        a[3] = rot_lerp_rad(w, a[3], -PI * 4.0 / 5.0) + w * 0.35 * o;
        a[5] = rot_lerp_rad(w, a[5], -0.15);
    });
    p.set("left_arm", |a| {
        a[3] = rot_lerp_rad(w, a[3], -PI * 4.0 / 5.0) - w * 0.35 * o;
        a[5] = rot_lerp_rad(w, a[5], 0.15);
    });
    p.set("left_leg", |l| l[3] -= w * 0.55 * o);
    p.set("right_leg", |l| l[3] += w * 0.55 * o);
    p.set("head", |h| h[3] = 0.0);
}

fn idle_and_crouch(p: &mut Pose<'_>, s: &AnimState) {
    if s.crouching {
        p.set("body", |b| {
            b[3] = 0.5;
            b[1] += 3.2;
        });
        for arm in ["right_arm", "left_arm"] {
            p.set(arm, |a| {
                a[3] += 0.4;
                a[1] += 3.2;
            });
        }
        for leg in ["right_leg", "left_leg"] {
            p.set(leg, |l| l[2] += 4.0);
        }
        p.set("head", |h| h[1] += 4.2);
    }
    for (k, (arm, f)) in [("right_arm", 1.0f32), ("left_arm", -1.0)].into_iter().enumerate() {
        if s.arm_pose[k] == arm::SPYGLASS {
            continue;
        }
        p.set(arm, |a| {
            a[5] += f * ((s.age * 0.09).cos() * 0.05 + 0.05);
            a[3] += f * (s.age * 0.067).sin() * 0.05;
        });
    }
}

/// Skeletons (skeleton, stray, bogged, parched, wither): same arm rendering.
fn is_skeleton(k: &str) -> bool {
    matches!(k, "skeleton" | "stray" | "bogged" | "parched" | "wither_skeleton")
}

/// Model-specific frame under the head, for the item worn on the head: copper golem
/// raised by 2 pixels and enlarged by 1/16, the others unchanged.
fn head_item_frame(kind: &str) -> Mat4 {
    if kind == "copper_golem" {
        Mat4::from_translation(Vec3::new(0.0, 0.125, 0.0)) * Mat4::from_scale(Vec3::splat(1.0625))
    } else {
        Mat4::IDENTITY
    }
}

/// Offset of the model of a block placed on the copper golem's antenna: flower beds
/// (pink petals, wildflowers) re-centred by a quarter block in x and z.
fn antenna_block_offset(flower_bed: bool) -> Mat4 {
    if flower_bed { Mat4::from_translation(Vec3::new(0.25, 0.0, 0.25)) } else { Mat4::IDENTITY }
}

/// Arm of the main layer seen by held items: frame of its parent, its pose, frame of the
/// body and X rotation of the right arm (allay hold).
#[derive(Clone, Copy)]
struct HandRef {
    parent: Mat4,
    arm: [f32; 9],
    body: Option<Mat4>,
    right_arm_x: f32,
}

impl HandRef {
    /// Frame of the arm (visible or not).
    fn tip(&self) -> Mat4 {
        self.parent * part_matrix(&self.arm)
    }
}

/// Frame of a part under `m` ignoring visibility (parents included).
fn raw_world(baked: &BakedLayer, parts: &[[f32; 9]], m: Mat4, i: usize) -> Mat4 {
    let p = baked.parts[i].parent;
    let parent = if p == u32::MAX { m } else { raw_world(baked, parts, m, p as usize) };
    parent * part_matrix(&parts[i])
}

/// Hand frame before the common placement of the held item, depending on the model: tip of the arm
/// (skeletons: pivot offset by one pixel outward; slim avatar: by half a pixel);
/// allay: under the body, turned like the right arm, scaled to 0.7; vex: arm scaled to 0.55;
/// copper golem: quarter turn at rest, otherwise scaled to 0.55 and brought against the body.
fn hand_frame(kind: &str, right: bool, h: &HandRef, slim: bool, golem_idle: bool) -> Mat4 {
    let s = if right { 1.0 } else { -1.0 };
    let shifted = |dx: f32| {
        let mut a = h.arm;
        a[0] += dx;
        h.parent * part_matrix(&a)
    };
    match kind {
        "allay" => {
            h.body.unwrap_or(h.parent)
                * Mat4::from_translation(Vec3::new(0.0, 0.0625, 0.1875))
                * Mat4::from_rotation_x(h.right_arm_x)
                * Mat4::from_scale(Vec3::splat(0.7))
                * Mat4::from_translation(Vec3::new(0.0625, 0.0, 0.0))
        }
        "vex" => {
            h.tip()
                * Mat4::from_scale(Vec3::splat(0.55))
                * Mat4::from_translation(Vec3::new(s * 0.046875, -0.15625, 0.078125))
        }
        "copper_golem" if golem_idle => {
            h.tip() * Mat4::from_rotation_y(deg(-90.0 * s)) * Mat4::from_translation(Vec3::new(0.0, 0.0, 0.125))
        }
        "copper_golem" => {
            h.tip() * Mat4::from_scale(Vec3::splat(0.55)) * Mat4::from_translation(Vec3::new(-0.125, 0.3125, -0.1875))
        }
        k if is_skeleton(k) => shifted(s),
        "player" | "mannequin" if slim => shifted(0.5 * s),
        _ => h.tip(),
    }
}

/// Arms of an aggressive skeleton without a bow, after the humanoid animation: arms stretched
/// forward, spread then folded back by the swing, idle sway.
fn skeleton_arms(p: &mut Pose<'_>, s: &AnimState, aggressive: bool) {
    if !aggressive || s.arm_pose.contains(&arm::BOW) {
        return;
    }
    let swing2 = (s.swing * PI).sin();
    let swing = ((1.0 - (1.0 - s.swing) * (1.0 - s.swing)) * PI).sin();
    let x = -PI / 2.0 - (swing2 * 1.2 - swing * 0.4);
    let y = 0.1 - swing2 * 0.6;
    for (arm, f) in [("right_arm", -1.0f32), ("left_arm", 1.0)] {
        p.set(arm, |a| {
            a[5] = 0.0;
            a[4] = f * y;
            a[3] = x;
        });
    }
    for (arm, f) in [("right_arm", 1.0f32), ("left_arm", -1.0)] {
        p.set(arm, |a| {
            a[5] += f * ((s.age * 0.09).cos() * 0.05 + 0.05);
            a[3] += f * (s.age * 0.067).sin() * 0.05;
        });
    }
}

/// Zombie arms (§ 2.2), after the humanoid animation.
fn zombie_arms(p: &mut Pose<'_>, s: &AnimState, aggressive: bool, raise: bool) {
    let fall = if raise { -PI / if aggressive { 1.5 } else { 2.25 } } else { 0.0 };
    let ay = (if raise { 1.0 } else { -1.0 }) * (s.swing * PI).sin();
    let ax = ((1.0 - (1.0 - s.swing).powi(2)) * PI).sin();
    let x = fall + ay * 1.2 - ax * 0.4;
    let y = 0.1 - ay * 0.6;
    // A held spear or a lunge leaves both arms in their walking pose.
    if s.arm_pose.contains(&arm::SPEAR) {
        idle_and_crouch(p, &AnimState { crouching: false, ..*s });
        return;
    }
    // An arm in "trident" pose (drowned) is not overwritten.
    if s.arm_pose[0] != arm::TRIDENT {
        p.set("right_arm", |a| {
            a[3] = x;
            a[4] = if raise { -y } else { y };
            a[5] = 0.0;
        });
    }
    if s.arm_pose[1] != arm::TRIDENT {
        p.set("left_arm", |a| {
            a[3] = x;
            a[4] = if raise { y } else { -y };
            a[5] = 0.0;
        });
    }
    idle_and_crouch(p, &AnimState { crouching: false, ..*s });
}

fn quadruped(p: &mut Pose<'_>, s: &AnimState) {
    quadruped_legs(p, s, false);
}

/// Quadruped; `crossed`: left and right legs swapped (creeper of the 26.3 game, which
/// links each leg to the part on the opposite side: reversed phases, quirk reproduced).
fn quadruped_legs(p: &mut Pose<'_>, s: &AnimState, crossed: bool) {
    p.set("head", |h| {
        h[3] = s.pitch;
        h[4] = s.yaw;
    });
    let q = s.walk_pos * 0.6662;
    let (a, b) = (q.cos() * 1.4 * s.walk_speed, (q + PI).cos() * 1.4 * s.walk_speed);
    let (a, b) = if crossed { (b, a) } else { (a, b) };
    p.set("right_hind_leg", |l| l[3] = a);
    p.set("left_front_leg", |l| l[3] = a);
    p.set("left_hind_leg", |l| l[3] = b);
    p.set("right_front_leg", |l| l[3] = b);
}

fn spider(p: &mut Pose<'_>, s: &AnimState) {
    p.set("head", |h| {
        h[3] = s.pitch;
        h[4] = s.yaw;
    });
    let q = s.walk_pos * 0.6662;
    let v = s.walk_speed;
    let g = [
        -(2.0 * q).cos() * 0.4 * v,
        -(2.0 * q + PI).cos() * 0.4 * v,
        -(2.0 * q + PI / 2.0).cos() * 0.4 * v,
        -(2.0 * q + 3.0 * PI / 2.0).cos() * 0.4 * v,
    ];
    let l = [
        (q.sin() * 0.4).abs() * v,
        ((q + PI).sin() * 0.4).abs() * v,
        ((q + PI / 2.0).sin() * 0.4).abs() * v,
        ((q + 3.0 * PI / 2.0).sin() * 0.4).abs() * v,
    ];
    let pairs = [
        ("right_hind_leg", "left_hind_leg"),
        ("right_middle_hind_leg", "left_middle_hind_leg"),
        ("right_middle_front_leg", "left_middle_front_leg"),
        ("right_front_leg", "left_front_leg"),
    ];
    for (k, (r, lf)) in pairs.iter().enumerate() {
        p.set(r, |x| {
            x[4] += g[k];
            x[5] += l[k];
        });
        p.set(lf, |x| {
            x[4] -= g[k];
            x[5] -= l[k];
        });
    }
}

/// Default animation of an unknown model: head, then limbs from their names.
fn generic(p: &mut Pose<'_>, s: &AnimState) {
    if p.has("right_arm") && p.has("right_leg") {
        humanoid(p, s);
    } else {
        quadruped(p, s);
    }
}

/// Render type of a layer.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    Cutout,
    Translucent,
    /// Emissive (eyes): no lightmap or overlay, no depth write.
    Eyes,
    /// Energy swirl (wither armor): additive, unlit, scrolling texture.
    Swirl,
    /// Cutout with back-face culling (bat, arrow, chest, bobber…).
    CutoutCull,
    /// Opaque without cutout, back faces culled (cape, shield, trident, banner, bell,
    /// book, decorated pot, conduit shell).
    Solid,
    /// Translucent emissive (glowing layers of the warden, breeze eyes): no lightmap,
    /// cardinal lighting and overlay kept.
    Emissive,
    /// Breeze wind: translucent without cardinal lighting or overlay, scrolling texture.
    Wind,
}

/// Instance passes (indices of `instances`): 0 cutout, 1 translucent, 2 eyes, 3 water
/// mask, 4 additive (lightning), 5 dissolve, 6 glint in a separate pass, 7 swirl, 8 culled
/// cutout (including cutout faces of items, carried or falling non-translucent blocks), 9 opaque,
/// 10 culled translucent (ghost of an invisible entity), 11 translucent drawn with the
/// solids (improved transparency only: special item models, whose blended banner
/// and shield patterns stay in the game's solid phase), 12 translucent items and blocks
/// (translucent faces of items, carried or falling translucent blocks), back faces
/// culled.
const PASSES: usize = 13;
const PASS_CUTOUT_CULL: usize = 8;
const PASS_SOLID: usize = 9;
const PASS_TRANSLUCENT_CULL: usize = 10;
const PASS_TRANSLUCENT_SOLID: usize = 11;
const PASS_ITEM_TRANSLUCENT: usize = 12;
/// Source of an instance of [`PASS_ITEM_TRANSLUCENT`] (high bit): block mesh, distance
/// taken at the centre of the cube; otherwise item, distance taken at the origin of its pose.
const SRC_BLOCK_CENTER: u8 = 0x10;
/// Drawing order: opaques (cutouts, opaques, dissolve), then translucents and effects. In
/// classic rendering, the game's translucent phase draws the translucent items and moving blocks
/// (sorted among themselves from farthest to nearest), then the translucent entity models
/// (including the ghost of an invisible entity), then the free geometry.
const PASS_ORDER: [usize; PASSES] = [
    0,
    PASS_CUTOUT_CULL,
    PASS_SOLID,
    5,
    PASS_TRANSLUCENT_SOLID,
    PASS_ITEM_TRANSLUCENT,
    1,
    PASS_TRANSLUCENT_CULL,
    2,
    3,
    4,
    6,
    7,
];
/// Pass of a special item model (`kind` = index + 1 in `SPECIAL_KINDS`). Only the heads
/// (kinds 1 and 2, the game's "translucent entity" type) are blended: they stay in the
/// translucent pass 1 in classic rendering. All the others (chest, banner, shield, trident...) are
/// in the game's solid phase, so drawn before the translucent items and blocks (pass 12):
/// a glass in front of a chest must not hide it by its depth.
fn special_pass(oit: bool, kind: u16) -> usize {
    if oit || kind > 2 { PASS_TRANSLUCENT_SOLID } else { 1 }
}

/// Passes whose geometry counts for the silhouette of glowing entities.
const GLOW_PASSES: [usize; 7] =
    [0, 1, PASS_CUTOUT_CULL, PASS_SOLID, PASS_TRANSLUCENT_CULL, PASS_TRANSLUCENT_SOLID, PASS_ITEM_TRANSLUCENT];
/// Improved transparency: passes drawn in the solids pass (glint in a separate pass
/// included); the water mask is separate, the rest follows the stages.
const OIT_SOLID_PASSES: [usize; 6] = [0, PASS_CUTOUT_CULL, PASS_SOLID, 5, PASS_TRANSLUCENT_SOLID, 6];

/// Current glowing entity: colour, end of the instances already placed in each pass of
/// [`GLOW_PASSES`], end of the silhouette-only ones.
type Glowing = (u32, [usize; GLOW_PASSES.len()], usize);

impl Pass {
    /// Index of the instance pass.
    fn index(self) -> usize {
        match self {
            Pass::Cutout => 0,
            Pass::Translucent => 1,
            Pass::Eyes => 2,
            Pass::Swirl => 7,
            Pass::CutoutCull => PASS_CUTOUT_CULL,
            Pass::Solid => PASS_SOLID,
            Pass::Emissive | Pass::Wind => 1,
        }
    }
}

const DYE_COLORS: [u32; 16] = [
    0xF9FFFE, 0xF9801D, 0xC74EBD, 0x3AB3DA, 0xFED83D, 0x80C71F, 0xF38BAA, 0x474F52, 0x9D9D97, 0x169C9C, 0x8932B8,
    0x3C44AA, 0x835432, 0x5E7C16, 0xB02E26, 0x1D1D21,
];

fn wool_color(i: usize) -> u32 {
    if i == 0 {
        return 0xffe6_e6e6;
    }
    let c = DYE_COLORS[i % 16];
    let ch = |s: u32| (((c >> s) & 0xff) as f32 * 0.75) as u32;
    0xff00_0000 | ch(16) << 16 | ch(8) << 8 | ch(0)
}

/// Default skins (`items.md` part 3 § 1.4.11): 9 slim then 9 wide.
pub(crate) const SKINS: [&str; 9] = ["alex", "ari", "efe", "kai", "makena", "noor", "steve", "sunny", "zuri"];

pub(crate) fn default_skin(uuid: u128) -> u8 {
    let (hi, lo) = ((uuid >> 64) as u64, uuid as u64);
    let x = hi ^ lo;
    let h = ((x >> 32) as i32) ^ (x as i32);
    h.rem_euclid(18) as u8
}

/// Default skin of a player or a mannequin. The mannequin draws it from its profile, not from
/// its own id: profile id, otherwise that of an offline player of that name,
/// otherwise the null id (empty profile, start value not sent: same skin for everyone).
fn avatar_default_skin(e: &Entity) -> u8 {
    if e.kind != "mannequin" {
        return default_skin(e.uuid);
    }
    default_skin(mannequin_profile(e).map_or(0, |p| p.uuid))
}

/// Sniffer: layer and texture; the baby has its own texture ("snifflet"). The game draws
/// the baby with the adult model class: no static transformation of the head.
fn sniffer_layer(baby: bool) -> (&'static str, &'static str) {
    if baby {
        ("minecraft:sniffer_baby#main", "entity/sniffer/snifflet")
    } else {
        ("minecraft:sniffer#main", "entity/sniffer/sniffer")
    }
}

/// Cape and elytra of an avatar (see [`EntityRenderer::avatar_extras`]), by fields separated
/// from the engine so as to serve while another of its tables is borrowed.
fn avatar_extras(
    atlas: &mut crate::entity_atlas::EntityAtlas,
    capes: &HashMap<u128, [f32; 4]>,
    elytras: &HashMap<u128, [f32; 4]>,
    e: &Entity,
) -> (Option<[f32; 4]>, Option<[f32; 4]>) {
    match e.kind.as_str() {
        "player" => (capes.get(&e.uuid).copied(), elytras.get(&e.uuid).copied()),
        "mannequin" => {
            let Some(p) = mannequin_profile(e) else { return (None, None) };
            let key = p.textures.as_deref().map(textures_key);
            let mut patched = |id: &Option<String>| {
                id.as_deref().and_then(|id| atlas.rect(id.strip_prefix("minecraft:").unwrap_or(id)))
            };
            let cape = patched(&p.cape).or_else(|| key.and_then(|k| capes.get(&k).copied()));
            let elytra = patched(&p.elytra).or_else(|| key.and_then(|k| elytras.get(&k).copied()));
            (cape, elytra)
        }
        _ => (None, None),
    }
}

/// "Cape" part shown for an avatar (player or mannequin: bit 0 of the skin parts).
fn shows_cape(e: &Entity) -> bool {
    matches!(e.kind.as_str(), "player" | "mannequin") && e.meta_byte(16) & 1 != 0
}

/// Own elytra texture of an avatar, (cape, elytra) of its skin: the elytra if the skin
/// has one (cape shown or not), otherwise the cape if it is shown.
fn wings_texture((cape, elytra): (Option<[f32; 4]>, Option<[f32; 4]>), cape_shown: bool) -> Option<[f32; 4]> {
    elytra.or_else(|| cape.filter(|_| cape_shown))
}

/// Network profile of a mannequin (profile metadata), read.
fn mannequin_profile(e: &Entity) -> Option<AvatarProfile> {
    e.meta.values().find_map(|m| match m {
        rc_proto_v777::entity::Meta::Profile(b) => parse_profile(b),
        _ => None,
    })
}

/// Id carried by a network profile (`either(full profile, partial profile)` followed by the
/// skin patches): full = id; partial = id, otherwise offline name.
#[cfg(test)]
fn profile_uuid(bytes: &[u8]) -> Option<u128> {
    parse_profile(bytes).map(|p| p.uuid)
}

/// Mannequin profile read: id, `textures` property (base 64) and skin patch.
/// The game renders this profile as is, without an online lookup, except a partial one without a property that
/// carries only a name or an id (lookup not done here: default skin).
#[derive(Default, Debug, PartialEq)]
struct AvatarProfile {
    uuid: u128,
    textures: Option<String>,
    /// Patch: body texture, cape, elytra (resource ids), slim model.
    body: Option<String>,
    cape: Option<String>,
    elytra: Option<String>,
    slim: Option<bool>,
}

fn parse_profile(bytes: &[u8]) -> Option<AvatarProfile> {
    use rc_proto::Decode;
    let r = &mut rc_proto::Reader::new(bytes);
    let uuid = |r: &mut rc_proto::Reader<'_>| -> Option<u128> { Some(u128::from_be_bytes(r.take_array().ok()?)) };
    // Properties: n × (name, value, optional signature); returns the first `textures`.
    fn props(r: &mut rc_proto::Reader<'_>) -> Option<Option<String>> {
        use rc_proto::Decode;
        let n = rc_proto::read_count(r, 16).ok()?;
        let mut textures = None;
        for _ in 0..n {
            let name = rc_proto::read_str(r, 64).ok()?;
            let value = rc_proto::read_str(r, 32767).ok()?;
            if bool::decode(r).ok()? {
                rc_proto::read_str(r, 1024).ok()?;
            }
            if name == "textures" && textures.is_none() {
                textures = Some(value.to_owned());
            }
        }
        Some(textures)
    }
    let mut p = AvatarProfile::default();
    if bool::decode(r).ok()? {
        p.uuid = uuid(r)?;
        rc_proto::read_str(r, 16).ok()?;
        p.textures = props(r)?;
    } else {
        let name = if bool::decode(r).ok()? { Some(rc_proto::read_str(r, 16).ok()?.to_owned()) } else { None };
        let id = if bool::decode(r).ok()? { Some(uuid(r)?) } else { None };
        p.uuid = id.or_else(|| name.map(|n| rc_client::offline_uuid(&n))).unwrap_or(0);
        // The bytes of a truncated profile say nothing more: the id remains valid.
        let Some(t) = props(r) else { return Some(p) };
        p.textures = t;
    }
    // Skin patch: four optional fields, tolerant of a truncated end.
    let opt_str = |r: &mut rc_proto::Reader<'_>| -> Option<Option<String>> {
        Some(if bool::decode(r).ok()? { Some(rc_proto::read_str(r, 32767).ok()?.to_owned()) } else { None })
    };
    p.body = opt_str(r).flatten();
    p.cape = opt_str(r).flatten();
    p.elytra = opt_str(r).flatten();
    p.slim = bool::decode(r).ok().and_then(|has| if has { bool::decode(r).ok() } else { None });
    Some(p)
}

/// Key of the downloaded skin of a static profile: fingerprint of its `textures` property (high
/// bit set, like the heads).
fn textures_key(t: &str) -> u128 {
    let mut h: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    for b in t.bytes() {
        h ^= u128::from(b);
        h = h.wrapping_mul(0x0000_0000_0100_0000_0000_0000_0000_013B);
    }
    h | 1 << 127
}

/// Batch of a special item model: tint, texture rectangle in the atlas (pixels) and
/// vertex range (UV specific to the model).
#[derive(Clone)]
struct SpecialGroup {
    color: u32,
    rect: [f32; 4],
    first: u32,
    count: u32,
}

/// Layer ready to draw: baked layer and its range in the static buffer.
struct GpuLayer {
    name: String,
    baked: BakedLayer,
    first: u32,
    count: u32,
    /// `equipment_layer(name)` and `outlined_when_invisible(name)`, computed at baking.
    equipment: bool,
    outlined: bool,
}

/// Sizes of an entity for the frame: base (`Entity::base_size`) and full (`Entity::size`).
type Sizes = (Size, Size);
/// Kept probe at the eyes' block: exact position (bits), (block, sky).
type EyeProbe = ([u64; 3], (u8, u8));

/// Optimisation switches of the entity rendering, read once at engine creation;
/// `RUSTCRAFT_OPT_<NAME>=0` restores the old path. Public fields in the module: the equivalence
/// proofs compare the two paths in a single process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EntOpt {
    /// `EQUIPFLAG`: layer flags baked instead of per-frame substring searches,
    /// product by the layer offset skipped when it is the identity, water fluids mask
    /// computed once.
    pub equip_flag: bool,
    /// `KINDINFO`: kind flags read from a table by type id.
    pub kind_info: bool,
    /// `SIZEONCE`: entity size computed once per frame.
    pub size_once: bool,
    /// `OBJEARLY`: kind tested before the light probe and the object matrix.
    pub obj_early: bool,
    /// `LIGHTONCE`: light probe at the eyes' block done once per entity and per frame.
    pub light_once: bool,
    /// `REGIDX`: item registry found once; `ITEMFLAGS`: flags per item.
    pub reg_idx: bool,
    pub item_flags: bool,
}

impl EntOpt {
    /// Old paths everywhere.
    #[cfg(test)]
    pub const OLD: EntOpt = EntOpt {
        equip_flag: false,
        kind_info: false,
        size_once: false,
        obj_early: false,
        light_once: false,
        reg_idx: false,
        item_flags: false,
    };

    fn from_env() -> EntOpt {
        let on = |v: Option<&str>| v != Some("0");
        EntOpt {
            equip_flag: on(rc_data::env_str!("RUSTCRAFT_OPT_EQUIPFLAG")),
            kind_info: on(rc_data::env_str!("RUSTCRAFT_OPT_KINDINFO")),
            size_once: on(rc_data::env_str!("RUSTCRAFT_OPT_SIZEONCE")),
            obj_early: on(rc_data::env_str!("RUSTCRAFT_OPT_OBJEARLY")),
            light_once: on(rc_data::env_str!("RUSTCRAFT_OPT_LIGHTONCE")),
            reg_idx: on(rc_data::env_str!("RUSTCRAFT_OPT_REGIDX")),
            item_flags: on(rc_data::env_str!("RUSTCRAFT_OPT_ITEMFLAGS")),
        }
    }
}

/// Resolved layer for an entity kind: layer, texture, pass; the wool tint is
/// computed separately.
#[derive(Clone, Copy)]
struct Resolved {
    layer: usize,
    rect: [f32; 4],
    pass: Pass,
    wool: Wool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Wool {
    None,
    /// Undercoat (adult, non-white colour).
    Undercoat,
    /// Wool (if not sheared).
    Coat,
    /// Dyeable body armor: tint of the worn item (body slot), otherwise this
    /// default colour; 0 → layer hidden.
    BodyDye(u32),
    /// ARGB tint computed by the family (`tint`), free channel.
    Channel(u8),
    /// Fixed ARGB tint (collar colour, known when the layers are resolved).
    Fixed(u32),
    /// White with variable alpha (`pulse_alpha`): glowing layers of the warden, eyes of the creaking;
    /// layer skipped below 1e-5.
    Pulse(u8),
}

pub struct EntityRenderer {
    layers: Vec<GpuLayer>,
    layer_index: HashMap<String, usize>,
    atlas: crate::entity_atlas::EntityAtlas,
    /// Layers resolved per (kind, baby, skin): avoids strings every frame.
    resolved: HashMap<(i32, bool, u32), usize>,
    /// Resolved armor layers: (base model, slot, item, baby) → index.
    armor_index: HashMap<(u32, u8, i32, bool), usize>,
    /// Downloaded skins: player UUID → layers in the arena.
    custom_skins: HashMap<u128, usize>,
    /// Retouched mannequin layers, per (texture rectangle, slim model).
    avatar_layers: HashMap<([u32; 4], bool), usize>,
    /// Downloaded capes: player UUID → rectangle in the atlas.
    custom_capes: HashMap<u128, [f32; 4]>,
    /// Downloaded elytra (profile ELYTRA texture): UUID → rectangle in the atlas.
    custom_elytras: HashMap<u128, [f32; 4]>,
    armor_arena: Vec<Vec<ArmorLayer>>,
    /// Work buffers of held items (layers, key).
    held_scratch: (Vec<rc_assets::item::Layer>, Vec<u64>),
    /// Pose of the main layer of the current entity (parameters, visibility).
    base_pose: (Vec<[f32; 9]>, Vec<bool>),
    /// (World) matrices of the parts of the main layer of the current entity.
    base_world: Vec<Option<Mat4>>,
    arena: Vec<Vec<Resolved>>,
    /// States with a full collision shape (shadow support), by id.
    full_cube: Vec<bool>,
    /// Piston heads and bases (moving blocks).
    piston_states: rc_physics::piston::PistonStates,
    /// Shadow radius and strength per entity kind, filled on first use.
    shadow_kinds: Vec<Option<(f32, f32)>>,
    has_shadow: bool,
    /// Setting `entityShadows`.
    pub shadows: bool,
    /// Scale of display distances (`clamp(distance / 8, 1, 2.5) × entityDistanceScaling`).
    pub view_scale: f64,
    /// Dimensions per entity type (width, height, eyes, label).
    pub dims: Vec<[f32; 4]>,
    /// Work buffers reused from one entity to the next.
    scratch: (Vec<[f32; 9]>, Vec<bool>, Vec<Option<Mat4>>),
    /// Optimisation switches (read once at creation).
    pub(crate) opt: EntOpt,
    /// `water_fluids(source)`, computed once.
    water_bits: u64,
    /// Kind flags per type id (`KindInfo`), names of the type registry.
    kinds: Vec<(String, KindInfo)>,
    /// Entity currently in `prepare` (address, 0 outside `prepare`): base and full sizes,
    /// and the light probe at the eyes' block (exact position, block, sky) once done.
    cur_addr: usize,
    cur_size: Option<Sizes>,
    cur_light: std::cell::Cell<Option<EyeProbe>>,
    /// Index of the `minecraft:item` registry in `source.registries`, flags per item.
    item_reg: Option<usize>,
    item_bits: Vec<u8>,
    /// Glowing entities of the frame: identity (UUID) → RGB colour of the outline; set by
    /// the caller before `prepare`.
    pub glow: HashMap<u128, u32>,
    /// Copied instances of the glowing entities (source, first vertex, count, instance) and their
    /// batches (source, vertices, first instance, count) in the instance buffer.
    glow_list: Vec<(u8, u32, u32, Instance)>,
    glow_batches: Vec<(u8, std::ops::Range<u32>, u32, u32)>,
    outline: wgpu::RenderPipeline,
    cutout: wgpu::RenderPipeline,
    translucent: wgpu::RenderPipeline,
    eyes: wgpu::RenderPipeline,
    water_mask: wgpu::RenderPipeline,
    /// Additive blend (lightning).
    additive: wgpu::RenderPipeline,
    /// Cutout with dissolve (dying dragon).
    dissolve: wgpu::RenderPipeline,
    /// Enchantment glint in a separate pass (pass 6: special items with several layers):
    /// additive, equal depth, on the same geometry.
    glint_pass: wgpu::RenderPipeline,
    /// Culled cutout, culled opaque, culled translucent (passes 8 to 10).
    cutout_cull: wgpu::RenderPipeline,
    solid: wgpu::RenderPipeline,
    translucent_cull: wgpu::RenderPipeline,
    /// Energy swirl (pass 7): additive, depth written.
    swirl: wgpu::RenderPipeline,
    glint_started: Instant,
    /// Animated textures copied into the atlas (name, playback, current frame already written).
    anims: Vec<(String, crate::sprite_anim::SpriteAnim, bool)>,
    /// Settings `glintSpeed` and `glintStrength`.
    pub glint: (f32, f32),
    /// Free geometry: beams (cutout) and rays (additive, depth written).
    free_beam: wgpu::RenderPipeline,
    free_rays: wgpu::RenderPipeline,
    free_gbeam: wgpu::RenderPipeline,
    free_verts: [Vec<FreeVertex>; 3],
    free_buf: Option<(wgpu::Buffer, u64)>,
    /// End crystals of the frame (interpolated position, own time) for the dragon's beam.
    crystals: Vec<([f64; 3], f32)>,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    bind: Option<wgpu::BindGroup>,
    lights: wgpu::Buffer,
    statics: wgpu::Buffer,
    /// Part matrices of the frame (storage) and instances (vertices per instance).
    mats_buf: Option<(wgpu::Buffer, u64)>,
    inst_buf: Option<(wgpu::Buffer, u64)>,
    mats: Vec<PartMat>,
    /// Instances per pass: (source, first row of vertices, count, instance); source 0 =
    /// entity models, 1 = object meshes. Pass 3: boats' water mask
    /// (depth only); pass 4: additive (lightning); pass 5: dissolve (drawn just
    /// after the cutouts); see [`PASSES`].
    instances: [Vec<(u8, u32, u32, Instance)>; PASSES],
    /// Silhouettes without drawing: body of an invisible but glowing entity.
    glow_only: Vec<(u8, u32, u32, Instance)>,
    /// Invisible entities seen as a ghost by the local player (spectator, or same team that
    /// sees its invisible allies); set by the caller before `prepare`.
    pub ghosts: hashbrown::HashSet<u128>,
    /// Players in spectator mode (player list, and the local player): no armor, no held
    /// items, no flames, like the game; set by the caller before `prepare`.
    pub spectators: hashbrown::HashSet<u128>,
    /// World of the frame (dimension, time, sun, spawn, local death location) for the
    /// items carried outside the HUD, and camera entity (`None`: the local player); set
    /// by the caller before `prepare`.
    pub world: crate::stack::HudEnv,
    pub camera: Option<i32>,
    /// Players whose profile is named "Dinnerbone" or "Grumm" (rendered upside down if their
    /// cape is shown); set by the caller before `prepare`.
    pub upside_down: hashbrown::HashSet<u128>,
    /// "deadmau5" players (ears); set by the caller before `prepare`.
    pub extra_ears: hashbrown::HashSet<u128>,
    /// "nether" dimension (piglins and hoglins do not shake from zombification there).
    nether: std::cell::Cell<bool>,
    /// Hand in first person: start of its instances in each pass (added after
    /// those of the world) and first of its batches.
    hand_from: [usize; PASSES],
    hand_batch: usize,
    /// Interface models (after the hand): start of their instances, first of their batches,
    /// root matrix imposed while they are added (`special.rs`).
    gui_from: Option<[usize; PASSES]>,
    gui_batch: usize,
    gui_root: Option<Mat4>,
    /// Layer offset of the armors in the world (`render::view_layering`, around the eye).
    layering: Mat4,
    /// Layer offset towards the back (paintings, frame model), in the world.
    layering_forward: Mat4,
    /// Eye of the frame, in the frame of the world matrices (sorting of translucent items).
    eye: Vec3,
    /// Totem animation (after the screen effects, before the interface models): start of
    /// its instances, first of its batches, fixed lights of the 3D items and their binding.
    totem_from: Option<[usize; PASSES]>,
    totem_batch: usize,
    lights_totem: wgpu::Buffer,
    bind_totem: Option<wgpu::BindGroup>,
    /// Lights of the interface models (`special::entity_in_ui_lights`) and their binding.
    lights_ui: wgpu::Buffer,
    bind_ui: Option<wgpu::BindGroup>,
    /// Batches of the frame: (pass, source, vertices, first instance, count).
    batches: Vec<(usize, u8, std::ops::Range<u32>, u32, u32)>,
    /// Meshes of the item models (baked on first use): per model, groups
    /// (tint index, first vertex, count, at least one translucent face).
    item_meshes: HashMap<u32, Vec<(i32, u32, u32, bool)>>,
    /// Baked special models (kind + data [+ resolved skin]): groups (ABGR tint, first
    /// vertex, count) in the same buffer as the object meshes.
    special_meshes: HashMap<Vec<u32>, Vec<SpecialGroup>>,
    special: crate::item_special::SpecialAssets,
    /// Player heads seen as items: (identity, `textures` property) to request from the skins.
    pub pending_heads: Vec<(u128, String)>,
    asked_heads: hashbrown::HashSet<u128>,
    item_verts: Vec<StaticVertex>,
    item_buf: Option<(wgpu::Buffer, u64)>,
    item_dirty: bool,
    /// Camera orientation (camera-facing billboards): columns right, up, towards the camera.
    cam_rot: Mat4,
    /// Camera yaw (radians): rotation around Y turning +Z towards it (flames).
    cam_yaw: f32,
    ids: crate::stack::ComponentIds,
    source: Arc<VersionData>,
    equipment: Equipment,
    villager_hats: HashMap<String, u8>,
    /// Default states of the poppy (iron golem) and the carved pumpkin (snow golem).
    held_blocks: [Option<u32>; 4],
    /// Item → default state of the block of the same name (block on the copper golem's antenna).
    antenna_blocks: HashMap<i32, Option<u32>>,
    /// (item, fingerprint of the `block_state` component, 0 without) → content swallowed by a sulfur
    /// cube, skull or block state (`creatures::Swallowed`).
    swallowed: HashMap<(i32, u64), Option<creatures::Swallowed>>,
    variant_defs: HashMap<(String, String), VariantDef>,
    /// Received variants: registry → definition by network id.
    variants: HashMap<String, Vec<VariantDef>>,
    /// Default variant of each received registry (network id): the server does not send
    /// a variant left at its starting value.
    variant_default: HashMap<String, usize>,
    /// Entities drawn in the last frame.
    pub drawn: usize,
    /// Improved transparency active (set by the render): special models and solid faces of
    /// the items stored with the solids.
    pub oit: bool,
    /// Pipelines of the improved transparency (made on demand) and camera layout.
    oit_sets: Option<EntityOit>,
    camera_layout: wgpu::BindGroupLayout,
    /// Ground shadows: pipeline, vertices of the frame and buffer.
    shadow: wgpu::RenderPipeline,
    shadow_verts: Vec<ShadowTile>,
    shadow_buf: Option<(wgpu::Buffer, u64)>,
    shadow_count: u32,
    /// State of the non-living entities (block models, default items).
    objects: objects::Objects,
    /// Block-entity rendering state.
    pub blocks: blocks::BlockCache,
    /// Map textures in the atlas.
    map_tex: crate::maps::MapTextures,
}

/// Shadow vertex: position (camera frame), UV in the shadow texture, alpha.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ShadowTile {
    /// Bottom corner of the tile (relative to the camera's integer origin).
    pos: [f32; 3],
    /// Centre of the entity in the same frame, and 1 / (2 × radius).
    center: [f32; 2],
    inv: f32,
    alpha: f32,
}

/// What the shadows read from the world.
pub struct ShadowWorld<'a> {
    /// Block state.
    pub state: &'a dyn Fn(i32, i32, i32) -> u32,
    /// Maximum raw light (block, darkened sky) at the block.
    pub brightness: &'a dyn Fn(i32, i32, i32) -> i32,
    pub ambient: f32,
    /// Biome colours of a block (grass, foliage, dry foliage, water; RGB), unblended:
    /// tint of falling blocks.
    pub biome_tints: &'a dyn Fn(i32, i32, i32) -> [u32; 4],
}

/// Shadow radius and strength of an entity kind (the game's renderers).
/// Shadow radius factor specific to the entity (the base radius is cached per kind):
/// cubes × size, pufferfish 0.1 + 0.1 × inflation, otherwise age scale of baby mobs
/// (0.5 by default, specific to a few kinds) and extra reduction in some renderers.
/// Display distance of an entity (square `d2` of its distance to the camera), like the game:
/// mean side of the box × 64 × `view_scale`, with cases specific to some kinds.
pub(crate) fn within_render_distance(e: &Entity, d2: f64, view_scale: f64, dims: &[[f32; 4]]) -> bool {
    within_render_distance_of(e, d2, view_scale, || e.size(dims))
}

/// `within_render_distance` with the full size `full` (computed only if read).
fn within_render_distance_of(e: &Entity, d2: f64, view_scale: f64, full: impl Fn() -> Size) -> bool {
    // Mean side of the current box (size of cubes, babies, scale included).
    let size = || {
        let s = full();
        (f64::from(s.w) + f64::from(s.h) + f64::from(s.w)) / 3.0
    };
    let within = |s: f64| d2 < s * s;
    let young = e.age < 2 && d2 < 12.25;
    match e.kind.as_str() {
        "phantom" | "trident" => true,
        "player" | "arrow" | "spectral_arrow" => within(size() * 10.0 * 64.0 * view_scale),
        "armor_stand" => {
            let s = size() * 4.0;
            within(if s == 0.0 { 4.0 } else { s } * 64.0)
        }
        "item_frame" | "glow_item_frame" => within(16.0 * 64.0 * view_scale),
        "leash_knot" => d2 < 1024.0,
        "block_display" | "item_display" | "text_display" => {
            let range = match e.meta.get(&17) {
                Some(rc_proto_v777::entity::Meta::Float(v)) => f64::from(*v),
                _ => 1.0,
            };
            within(range * 64.0 * view_scale)
        }
        "lightning_bolt" => within(64.0 * view_scale),
        "firework_rocket" | "fishing_bobber" => d2 < 4096.0,
        "shulker_bullet" => d2 < 16384.0,
        "fireball" | "small_fireball" | "wind_charge" | "breeze_wind_charge" if young => false,
        "snowball" | "egg" | "ender_pearl" | "splash_potion" | "lingering_potion" | "experience_bottle"
        | "eye_of_ender"
            if young =>
        {
            false
        }
        "fireball" | "small_fireball" | "dragon_fireball" | "wither_skull" | "wind_charge" | "breeze_wind_charge"
        | "snowball" | "egg" | "ender_pearl" | "splash_potion" | "lingering_potion" | "experience_bottle"
        | "eye_of_ender" => within(size() * 4.0 * 64.0),
        "end_crystal" => objects::beam_target(e).is_some() || within(size() * 64.0 * view_scale),
        _ => within(size() * 64.0 * view_scale),
    }
}

/// Vertical render offset of a crouching avatar (−2/16 × scale), 0 otherwise. It affects the
/// model, the flames and the name tag, never the light probes or the shadow.
pub(crate) fn crouch_offset(e: &Entity) -> f64 {
    if matches!(e.kind.as_str(), "player" | "mannequin") && e.pose() == 5 {
        -f64::from(living_scale(e)) * 2.0 / 16.0
    } else {
        0.0
    }
}

/// Scale of a living entity: attribute `minecraft:scale` bounded according to the type (dragon
/// always 1, shulker 3 at most, happy ghast 1 at most).
pub(super) fn living_scale(e: &Entity) -> f32 {
    match e.kind.as_str() {
        "ender_dragon" => 1.0,
        "shulker" => e.scale.min(3.0),
        "happy_ghast" => e.scale.min(1.0),
        _ => e.scale,
    }
}

/// Shadow radius factor: size (cubes, pufferfish, without the scale attribute), otherwise
/// living scale × baby factor.
fn shadow_scale(e: &Entity) -> f32 {
    match e.kind.as_str() {
        "slime" | "magma_cube" | "sulfur_cube" => return e.meta_int(18).unwrap_or(1).clamp(1, 127) as f32,
        "pufferfish" => return 1.0 + e.meta_int(17).unwrap_or(0).clamp(0, 2) as f32,
        _ => {}
    }
    let attr = if e.class != Kind::Other { living_scale(e) } else { 1.0 };
    if matches!(e.kind.as_str(), "player" | "mannequin" | "armor_stand") || !e.baby() {
        return attr;
    }
    let (age, extra) = match e.kind.as_str() {
        "armadillo" | "camel" | "camel_husk" => (0.6, 1.0),
        "dolphin" => (0.65, 1.0),
        "goat" => (0.55, 1.0),
        "happy_ghast" => (0.2375, 1.0),
        "turtle" => (0.3, 0.83),
        "villager" | "strider" => (0.5, 0.5),
        _ => (0.5, 1.0),
    };
    age * extra * attr
}

/// Wearable skulls: (item, model, default texture).
const SKULLS: [(&str, &str, &str); 7] = [
    ("minecraft:skeleton_skull", "minecraft:skeleton_skull#main", "entity/skeleton/skeleton"),
    ("minecraft:wither_skeleton_skull", "minecraft:wither_skeleton_skull#main", "entity/skeleton/wither_skeleton"),
    ("minecraft:zombie_head", "minecraft:zombie_head#main", "entity/zombie/zombie"),
    ("minecraft:creeper_head", "minecraft:creeper_head#main", "entity/creeper/creeper"),
    ("minecraft:dragon_head", "minecraft:dragon_skull#main", "entity/enderdragon/dragon"),
    ("minecraft:piglin_head", "minecraft:piglin_head#main", "entity/piglin/piglin"),
    ("minecraft:player_head", "minecraft:player_head#main", "entity/player/slim/steve"),
];

fn shadow_of(kind: &str) -> (f32, f32) {
    if let Some(s) = animals::shadow(kind)
        .or_else(|| creatures::shadow(kind))
        .or_else(|| dragon::shadow(kind))
        .or_else(|| wither::shadow(kind))
        .or_else(|| shulker::shadow(kind))
        .or_else(|| objects::shadow(kind))
    {
        return s;
    }
    match kind {
        "item" | "experience_orb" => (0.15, 0.75),
        "chicken" | "parrot" | "bat" | "bee" | "silverfish" | "endermite" | "rabbit" | "frog" | "tadpole" => (0.3, 1.0),
        "cat" | "ocelot" | "fox" | "axolotl" => (0.4, 1.0),
        "cow" | "pig" | "sheep" | "mooshroom" | "goat" | "llama" | "trader_llama" | "polar_bear" | "panda"
        | "turtle" | "sniffer" | "camel" | "horse" | "donkey" | "mule" | "zombie_horse" | "skeleton_horse" => {
            (0.7, 1.0)
        }
        "spider" => (0.8, 1.0),
        "cave_spider" => (0.7, 1.0),
        "iron_golem" => (1.0, 1.0),
        "ghast" | "happy_ghast" => (2.0, 1.0),
        "armor_stand" | "evoker_fangs" => (0.0, 0.0),
        "giant" => (3.0, 1.0),
        "ravager" => (1.1, 1.0),
        "vex" => (0.3, 1.0),
        "zoglin" | "hoglin" => (0.7, 1.0),
        _ => (0.5, 1.0),
    }
}

/// Entity lights of the world (`entities.md` part 2 § 3.3).
fn world_lights(nether: bool) -> [[f32; 4]; 2] {
    let n = |v: Vec3| {
        let v = v.normalize();
        [v.x, v.y, v.z, 0.0]
    };
    [n(Vec3::new(0.2, 1.0, -0.7)), n(Vec3::new(-0.2, if nether { -1.0 } else { 1.0 }, 0.7))]
}

/// Fixed lights of 3D items outside the world (totem animation), in the view frame:
/// the two daylight lights rotated by the game's item pose (mirror in y, then two
/// Y-X rotations: 1.0821041 / 3.2375858 and −π/8 / 3π/4).
pub(crate) fn items_3d_lights() -> [[f32; 4]; 2] {
    use glam::Mat3;
    let pose = Mat3::from_diagonal(Vec3::new(1.0, -1.0, 1.0))
        * Mat3::from_rotation_y(1.0821041)
        * Mat3::from_rotation_x(3.2375858)
        * Mat3::from_rotation_y(-PI / 8.0)
        * Mat3::from_rotation_x(PI * 3.0 / 4.0);
    let l = world_lights(false);
    let t = |v: [f32; 4]| {
        let r = pose * Vec3::new(v[0], v[1], v[2]);
        [r.x, r.y, r.z, 0.0]
    };
    [t(l[0]), t(l[1])]
}

/// Uniform of group 1: two lights, the shadow texture rectangle, the item glint one,
/// the glint scroll (u and v offsets, strength), the armor glint rectangle.
fn lights_uniform(nether: bool, shadow: [f32; 4], glint: [f32; 4], glint_armor: [f32; 4]) -> [[f32; 4]; 6] {
    let l = world_lights(nether);
    [l[0], l[1], shadow, glint, [0.0, 0.0, 0.75, 0.0], glint_armor]
}

/// Glint scroll after `ms_since_start` milliseconds at speed setting `speed`:
/// (u, v) offsets of the texture matrix translation (period 110 s and 30 s at
/// speed 1/8, whole milliseconds like the game).
fn glint_offsets(ms_since_start: f64, speed: f32) -> (f32, f32) {
    let ms = (ms_since_start * f64::from(speed) * 8.0).floor() as i64;
    ((ms % 110_000) as f32 / 110_000.0, (ms % 30_000) as f32 / 30_000.0)
}

/// Glint mode of an instance, carried by bits 16-17 of the light word: 0 none;
/// 1 item mesh (atlas UV in pixels / 1024, scale 8); 2 entity model
/// (UV of its texture, scale 0.5); 3 worn armor (UV of its texture, scale 0.16, armor
/// glint texture).
pub(crate) const GLINT_ITEM: u32 = 1 << 16;
pub(crate) const GLINT_ENTITY: u32 = 2 << 16;
pub(crate) const GLINT_ARMOR: u32 = 3 << 16;
/// Flags of the light word read by the shader: emissive (no lightmap), no cardinal
/// lighting, scrolling texture (offset in the overlay word).
const LIGHT_EMISSIVE: u32 = 1 << 18;
const LIGHT_FLAT: u32 = 2 << 18;
const LIGHT_SCROLL: u32 = 4 << 18;
/// Special glint of compasses and the clock (with [`GLINT_ITEM`]): glint coordinates
/// projected per face from the vertex position in the model
/// ([`crate::item_atlas::special_glint_uv`], redone in the vertex shader).
const LIGHT_GLINT_SPECIAL: u32 = 8 << 18;
/// First-person context: special glint scale 0.75 (1 otherwise).
const LIGHT_FIRST_PERSON: u32 = 16 << 18;
/// Interface context (`gui`): special glint scale 0.5.
const LIGHT_GUI: u32 = 32 << 18;

/// Scale flag of the special glint for a display context (index of
/// `DISPLAY_CONTEXTS`): interface 0.5, first person 0.75, elsewhere 1 (no flag).
fn context_glint_bits(ctx: Option<usize>) -> u32 {
    match ctx {
        Some(0) => LIGHT_GUI,
        Some(4 | 5) => LIGHT_FIRST_PERSON,
        _ => 0,
    }
}

/// Glint word of an item layer: none, standard, or special (compasses, clock).
fn item_glint_bits(glint: u8) -> u32 {
    match glint {
        0 => 0,
        2 => GLINT_ITEM | LIGHT_GLINT_SPECIAL,
        _ => GLINT_ITEM,
    }
}

fn uniform_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    ty: wgpu::BufferBindingType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    }
}

impl EntityRenderer {
    pub fn new(
        device: &wgpu::Device,
        camera_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        data: Arc<VersionData>,
        tex: EntityTextures,
    ) -> Self {
        use wgpu::util::DeviceExt;
        keyframes::install(&data);
        let has_shadow = tex.sources.contains_key("misc/shadow");
        let mut atlas = crate::entity_atlas::EntityAtlas::new(tex.sources, Some(tex.assets));
        let shadow_rect = atlas.rect("misc/shadow").unwrap_or_default();
        // Read at startup (no resource read at runtime).
        let _ = (atlas.rect("#fire_0"), atlas.rect("#fire_1"));
        // All layers baked once into a static buffer.
        let mut statics: Vec<StaticVertex> = Vec::new();
        let mut layers = Vec::with_capacity(data.entity_models.len());
        let mut layer_index = HashMap::new();
        for l in &data.entity_models {
            let baked = bake_layer(l);
            let first = statics.len() as u32;
            for (pi, part) in baked.parts.iter().enumerate() {
                for q in &part.quads {
                    for k in [0, 1, 2, 0, 2, 3] {
                        statics.push(StaticVertex { pos: q.pos[k], uv: q.uv[k], normal: q.normal, part: pi as u32 });
                    }
                }
            }
            layer_index.insert(l.name.clone(), layers.len());
            layers.push(GpuLayer {
                name: l.name.clone(),
                baked,
                first,
                count: statics.len() as u32 - first,
                equipment: equipment_layer(&l.name),
                outlined: outlined_when_invisible(&l.name),
            });
        }
        let statics = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("entity models"),
            contents: bytemuck::cast_slice(&statics),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("entities"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let mut uniform = lights_uniform(
            false,
            shadow_rect,
            atlas.rect("#glint").unwrap_or([0.0, 0.0, 1.0, 1.0]),
            atlas.rect("#glint_armor").unwrap_or([0.0, 0.0, 1.0, 1.0]),
        );
        let lights = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("entity lights"),
            contents: bytemuck::cast_slice(&uniform),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        [uniform[0], uniform[1]] = items_3d_lights();
        let lights_totem = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("lights of 3D items"),
            contents: bytemuck::cast_slice(&uniform),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        [uniform[0], uniform[1]] = crate::special::entity_in_ui_lights();
        let lights_ui = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("lights of interface models"),
            contents: bytemuck::cast_slice(&uniform),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("entities"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    // Vertices: atlas size (rectangles in pixels).
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                uniform_entry(2, wgpu::ShaderStages::FRAGMENT, wgpu::BufferBindingType::Uniform),
                uniform_entry(3, wgpu::ShaderStages::VERTEX, wgpu::BufferBindingType::Storage { read_only: true }),
            ],
        });
        let shader = device.create_shader_module(wgpu::include_wgsl!("entity.wgsl"));
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("entities"),
            bind_group_layouts: &[Some(camera_layout), Some(&layout)],
            immediate_size: 0,
        });
        // Back-face culling (the game enables it by default; ordinary entity models
        // turn it off) and depth test.
        let pipeline_ex = |label,
                           fs: &str,
                           blend: Option<wgpu::BlendState>,
                           write: bool,
                           mask,
                           cull: bool,
                           compare: wgpu::CompareFunction| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[
                        Some(wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<StaticVertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x3, 3 => Uint32],
                        }),
                        Some(wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<Instance>() as u64,
                            step_mode: wgpu::VertexStepMode::Instance,
                            attributes: &wgpu::vertex_attr_array![
                                4 => Uint32, 5 => Float32x4, 6 => Unorm8x4, 7 => Uint32, 8 => Uint32
                            ],
                        }),
                    ],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: mask })],
                }),
                primitive: wgpu::PrimitiveState {
                    cull_mode: cull.then_some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(write),
                    depth_compare: Some(compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let le = crate::render::DEPTH_NEARER;
        let pipeline_masked = |label, fs: &str, blend: Option<wgpu::BlendState>, write: bool, mask| {
            pipeline_ex(label, fs, blend, write, mask, false, le)
        };
        let pipeline = |label, fs: &str, blend: Option<wgpu::BlendState>, write: bool| {
            pipeline_masked(label, fs, blend, write, wgpu::ColorWrites::ALL)
        };
        let culled = |label, fs: &str, blend: Option<wgpu::BlendState>, write: bool| {
            pipeline_ex(label, fs, blend, write, wgpu::ColorWrites::ALL, true, le)
        };
        let blend = Some(wgpu::BlendState::ALPHA_BLENDING);
        // Game "lightning" blend: (SrcAlpha, One) on colour AND alpha.
        let lightning = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let additive_blend = Some(wgpu::BlendState { color: lightning, alpha: lightning });
        // Glint in a separate pass: source colour × itself added, alpha kept.
        let glint_blend = Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Src,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        });
        // Free geometry: full vertices, no instance; depth tested and written.
        let free = |label, fs: &str, blend: Option<wgpu::BlendState>, cull: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_free"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<FreeVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x3, 1 => Float32x2, 2 => Float32x4, 3 => Unorm8x4, 4 => Uint32, 5 => Float32x3
                        ],
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: wgpu::PrimitiveState { cull_mode: cull.then_some(wgpu::Face::Back), ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(crate::render::DEPTH_NEARER),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        // Crystal beam without culling; dragon rays culled (back faces).
        let free_beam = free("entity beams", "fs_beam", None, false);
        let free_rays = free("entity rays", "fs_rays", additive_blend, true);
        let free_gbeam = free("guardian rays", "fs_gbeam", None, false);
        let shadow = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("entity shadows"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_shadow"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<ShadowTile>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32, 3 => Float32],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_shadow"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(crate::render::DEPTH_NEARER),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        // Silhouettes of the glowing entities: same vertices and instances, flat colour, no
        // depth, towards the outline target.
        let outline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("entity silhouettes"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<StaticVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x3, 3 => Uint32],
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Instance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            4 => Uint32, 5 => Float32x4, 6 => Unorm8x4, 7 => Uint32, 8 => Uint32
                        ],
                    }),
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_outline"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: crate::entity_outline::TARGET_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        EntityRenderer {
            glow: HashMap::new(),
            glow_list: Vec::new(),
            glow_batches: Vec::new(),
            outline,
            objects: objects::Objects::new(tex.blocks, tex.named_models, tex.grass_default),
            blocks: blocks::BlockCache::default(),
            shadow,
            shadow_verts: Vec::new(),
            shadow_buf: None,
            shadow_count: 0,
            layers,
            layer_index,
            atlas,
            equipment: tex.equipment,
            villager_hats: tex.villager_hats,
            held_blocks: [
                "minecraft:poppy",
                "minecraft:carved_pumpkin",
                "minecraft:red_mushroom",
                "minecraft:brown_mushroom",
            ]
            .map(|n| data.blocks.iter().find(|b| b.name == n).map(|b| b.default_state)),
            antenna_blocks: HashMap::new(),
            piston_states: rc_physics::piston::PistonStates::new(&data),
            swallowed: HashMap::new(),
            full_cube: data
                .states
                .iter()
                .map(|i| {
                    let sh = data.shape(i.collision);
                    sh.len() == 1 && sh[0].min == [0.0; 3] && sh[0].max == [1.0; 3]
                })
                .collect(),
            shadow_kinds: Vec::new(),
            has_shadow,
            shadows: true,
            view_scale: 1.0,
            dims: data.entity_dims.clone(),
            variant_defs: tex.variants,
            variants: HashMap::new(),
            variant_default: HashMap::new(),
            resolved: HashMap::new(),
            armor_index: HashMap::new(),
            custom_skins: HashMap::new(),
            avatar_layers: HashMap::new(),
            custom_capes: HashMap::new(),
            custom_elytras: HashMap::new(),
            armor_arena: Vec::new(),
            held_scratch: (Vec::new(), Vec::new()),
            base_pose: (Vec::new(), Vec::new()),
            base_world: Vec::new(),
            arena: Vec::new(),
            scratch: Default::default(),
            opt: EntOpt::from_env(),
            water_bits: water_fluids(&data),
            kinds: data
                .registry("minecraft:entity_type")
                .map(|r| {
                    r.entries
                        .iter()
                        .map(|n| {
                            let short = n.strip_prefix("minecraft:").unwrap_or(n).to_owned();
                            let info = KindInfo::of(&short);
                            (short, info)
                        })
                        .collect()
                })
                .unwrap_or_default(),
            cur_addr: 0,
            cur_size: None,
            cur_light: std::cell::Cell::new(None),
            item_reg: data.registries.iter().position(|r| r.name == "minecraft:item"),
            item_bits: data
                .registry("minecraft:item")
                .map(|r| r.entries.iter().map(|n| item_bits(n)).collect())
                .unwrap_or_default(),
            cutout: pipeline("cutout entities", "fs_cutout", None, true),
            translucent: pipeline("translucent entities", "fs_translucent", blend, true),
            // Eyes: back faces culled (the game's default setting for this type).
            eyes: culled("entity eyes", "fs_eyes", blend, false),
            // Water mask: depth only (shader without discard, no colour written).
            water_mask: pipeline_masked("water mask", "fs_eyes", None, true, wgpu::ColorWrites::empty()),
            // Lightning: depth written, back faces culled.
            additive: culled("additive entities", "fs_eyes", additive_blend, true),
            dissolve: pipeline("dissolving entities", "fs_dissolve", None, true),
            glint_started: Instant::now(),
            anims: tex.anims.into_iter().map(|(n, s)| (n, s, false)).collect(),
            glint: (0.5, 0.75),
            glint_pass: pipeline_ex(
                "enchantment glint",
                "fs_glint",
                glint_blend,
                false,
                wgpu::ColorWrites::ALL,
                false,
                wgpu::CompareFunction::Equal,
            ),
            cutout_cull: culled("culled cutout entities", "fs_cutout", None, true),
            solid: culled("opaque entities", "fs_solid", None, true),
            translucent_cull: culled("culled translucent entities", "fs_translucent", blend, true),
            swirl: pipeline("energy swirl", "fs_swirl", additive_blend, true),
            free_beam,
            free_rays,
            free_gbeam,
            free_verts: Default::default(),
            free_buf: None,
            crystals: Vec::new(),
            layout,
            sampler,
            bind: None,
            lights,
            statics,
            mats_buf: None,
            inst_buf: None,
            mats: Vec::new(),
            instances: Default::default(),
            hand_from: [0; PASSES],
            glow_only: Vec::new(),
            ghosts: hashbrown::HashSet::new(),
            spectators: hashbrown::HashSet::new(),
            world: crate::stack::HudEnv::empty(),
            camera: None,
            upside_down: hashbrown::HashSet::new(),
            extra_ears: hashbrown::HashSet::new(),
            nether: std::cell::Cell::new(false),
            hand_batch: 0,
            gui_from: None,
            gui_batch: 0,
            totem_from: None,
            totem_batch: 0,
            lights_totem,
            bind_totem: None,
            lights_ui,
            bind_ui: None,
            gui_root: None,
            layering: Mat4::IDENTITY,
            layering_forward: Mat4::IDENTITY,
            eye: Vec3::ZERO,
            batches: Vec::new(),
            item_meshes: HashMap::new(),
            special_meshes: HashMap::new(),
            special: tex.special,
            pending_heads: Vec::new(),
            asked_heads: hashbrown::HashSet::new(),
            item_verts: [orb_quad(), painting_tile(), maps::unit_quad()].concat(),
            map_tex: crate::maps::MapTextures::new((orb_quad().len() + painting_tile().len()) as u32),
            item_buf: None,
            item_dirty: true,
            cam_rot: Mat4::IDENTITY,
            cam_yaw: 0.0,
            ids: crate::stack::ComponentIds::new(&data),
            source: data.clone(),
            drawn: 0,
            oit: false,
            oit_sets: None,
            camera_layout: camera_layout.clone(),
        }
    }

    /// Variant registry received (order of the ids); omitted content is taken from the
    /// vanilla definitions.
    pub fn set_variants(&mut self, registry: &str, entries: &[(String, Option<rc_proto::nbt::Nbt>)]) {
        let defs = entries
            .iter()
            .map(|(name, nbt)| match nbt {
                Some(n) => VariantDef::from_nbt(n),
                None => self.variant_defs.get(&(registry.to_owned(), name.clone())).cloned().unwrap_or_default(),
            })
            .collect();
        self.variants.insert(registry.to_owned(), defs);
        let i = entries.iter().position(|(name, _)| name == default_variant(registry)).unwrap_or(0);
        self.variant_default.insert(registry.to_owned(), i);
        self.resolved.clear();
    }

    /// Definition of variant `index` of the registry; 255 = not received: the entity's
    /// starting variant (default key of the registry, otherwise the first entry).
    pub(crate) fn variant_def(&self, registry: &str, index: usize) -> Option<&VariantDef> {
        let i = if index == 255 { self.variant_default.get(registry).copied().unwrap_or(0) } else { index };
        self.variants.get(registry)?.get(i)
    }

    /// Size (width, height in blocks) of the received painting variant `variant`.
    pub fn painting_size(&self, variant: i32) -> Option<(u8, u8)> {
        let def = self.variants.get("minecraft:painting_variant")?.get(usize::try_from(variant).ok()?)?;
        Some((def.width, def.height))
    }

    /// Downloaded skin of a player (stable name `key`): replaces their default skin.
    /// Diagnostic: entity kinds without a resolved layer (adult, skin 0).
    pub fn unresolved(&mut self, names: &[String]) -> Vec<String> {
        names
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                let k = n.strip_prefix("minecraft:").unwrap_or(n);
                let idx = self.resolve(i as i32, k, false, 0);
                self.arena[idx].is_empty().then(|| k.to_owned())
            })
            .collect()
    }

    /// Downloaded textures of a profile (`uuid`: player or head key): skin (image, slim
    /// model, stable name), cape and elytra, each optional; without a skin, the default skin
    /// remains.
    pub fn set_skin(
        &mut self,
        uuid: u128,
        skin: Option<(rc_assets::texture::Image, bool, String)>,
        cape: Option<(rc_assets::texture::Image, String)>,
        elytra: Option<(rc_assets::texture::Image, String)>,
    ) {
        for (tex, map) in [(cape, &mut self.custom_capes), (elytra, &mut self.custom_elytras)] {
            if let Some((img, k)) = tex {
                self.atlas.insert(&k, img);
                if let Some(rect) = self.atlas.rect(&k) {
                    map.insert(uuid, rect);
                }
            }
        }
        let Some((image, slim, key)) = skin else { return };
        let key = key.as_str();
        self.atlas.insert(key, image);
        let model = if slim { "minecraft:player_slim#main" } else { "minecraft:player#main" };
        let (Some(&layer), Some(rect)) = (self.layer_index.get(model), self.atlas.rect(key)) else { return };
        self.arena.push(vec![Resolved { layer, rect, pass: Pass::Translucent, wool: Wool::None }]);
        self.custom_skins.insert(uuid, self.arena.len() - 1);
    }

    /// One advance of the texture animation, at the same moments as that of the blocks: current
    /// frame of the animated textures already placed (flames of burning entities, conduit wind,
    /// item sprites, blocks) rewritten into the atlas when it changes (or has just been
    /// placed).
    pub fn tick_textures(&mut self) {
        for (name, anim, written) in &mut self.anims {
            let changed = anim.tick();
            let Some(r) = self.atlas.placed(name) else { continue };
            if !changed && *written {
                continue;
            }
            if let Some(img) = anim.current(r[2] as u32).filter(|img| img.height == r[3] as u32) {
                self.atlas.write(r[0] as u32, r[1] as u32, img);
                *written = true;
            }
        }
    }

    /// Camera orientation (yaw, pitch in degrees) for the camera-facing billboards.
    pub fn set_camera(&mut self, yaw: f32, pitch: f32) {
        let yr = yaw.to_radians();
        let (right, up, dir) = crate::camera::axes(yaw, pitch);
        self.cam_rot = Mat4::from_cols(right.extend(0.0), up.extend(0.0), (-dir).extend(0.0), glam::Vec4::W);
        // Yaw part of the orientation (+Z towards the camera, horizontal).
        self.cam_yaw = std::f32::consts::PI - yr;
    }

    /// Painting (part C § 4.4): one tile per block, cutout front sprite, back and slices with the
    /// `back` sprite, rotation by direction, light per tile.
    fn push_painting(
        &mut self,
        e: &Entity,
        rel: [f32; 3],
        p: [f64; 3],
        light: &dyn Fn(i32, i32, i32) -> (u8, u8),
    ) -> bool {
        // Index 8: direction; 9: variant.
        let variant = e.meta_int(9).and_then(|v| usize::try_from(v).ok());
        let Some(def) = variant.and_then(|v| self.variants.get("minecraft:painting_variant")?.get(v)).cloned() else {
            return false;
        };
        let (Some(front), Some(back)) =
            (self.atlas.rect(&format!("painting/{}", def.asset)), self.atlas.rect("painting/back"))
        else {
            return false;
        };
        // 3D hanging direction (meta 8, otherwise spawn value) → horizontal (south 0,
        // west 1, north 2, east 3).
        let d = match e.attach_dir() {
            2 => 2,
            4 => 1,
            5 => 3,
            _ => 0,
        };
        let (w, h) = (i32::from(def.width), i32::from(def.height));
        // Received position = hanging block; centre = block centre moved back by 0.46875 towards the wall,
        // shifted half a block to the left (even width) and upwards (even height).
        let (fx, fz) = match d {
            2 => (0.0, -1.0),
            1 => (-1.0, 0.0),
            3 => (1.0, 0.0),
            _ => (0.0, 1.0),
        };
        // Left = direction turned by a quarter turn the opposite way (north → west).
        let (lx, lz) = (fz, -fx);
        let ho = if w % 2 == 0 { 0.5 } else { 0.0 };
        let vo = if h % 2 == 0 { 0.5 } else { 0.0 };
        let c = [
            p[0].floor() + 0.5 - fx * 0.46875 + lx * ho,
            p[1].floor() + 0.5 + vo,
            p[2].floor() + 0.5 - fz * 0.46875 + lz * ho,
        ];
        let rel = [rel[0] + (c[0] - p[0]) as f32, rel[1] + (c[1] - p[1]) as f32, rel[2] + (c[2] - p[2]) as f32];
        let p = c;
        let m = Mat4::from_translation(Vec3::from(rel)) * Mat4::from_rotation_y(deg(180.0 - d as f32 * 90.0));
        for sy in 0..h {
            for sx in 0..w {
                let (x0, y0) = (sx as f32 - w as f32 / 2.0, sy as f32 - h as f32 / 2.0);
                // Light at the block of the tile's centre (offset relative to the painting's centre).
                let (ox, oy) = (f64::from(x0) + 0.5, f64::from(y0) + 0.5);
                let (mut bx, mut bz) = (p[0], p[2]);
                match d {
                    2 => bx += ox,
                    1 => bz -= ox,
                    0 => bx -= ox,
                    _ => bz += ox,
                }
                let (bl, sl) = light(bx.floor() as i32, (p[1] + oy).floor() as i32, bz.floor() as i32);
                let packed = (u32::from(bl) * 16) | ((u32::from(sl) * 16) << 8);
                // The whole painting (face, back, slices) is shifted towards the back, like the game.
                let tm = self.layering_forward * m * Mat4::from_translation(Vec3::new(x0, y0, 0.0));
                let base = self.mats.len() as u32;
                self.mats.push(PartMat::new(&tm));
                // u decreasing as x grows, v decreasing as y grows.
                let cell = [
                    front[0] + front[2] * (w - sx - 1) as f32 / w as f32,
                    front[1] + front[3] * (h - sy - 1) as f32 / h as f32,
                    front[2] / w as f32,
                    front[3] / h as f32,
                ];
                let inst = |rect| Instance { base, rect, color: 0xffff_ffff, light: packed, overlay: 10 << 16 };
                self.instances[0].push((1, 6, 6, inst(cell)));
                self.instances[0].push((1, 12, 30, inst(back)));
            }
        }
        true
    }

    /// Experience orb (part C § 4.2): texture tile depending on the value, pulsing colour,
    /// camera-facing quad, translucent, block light + 7.
    fn push_orb(
        &mut self,
        e: &Entity,
        pt: f32,
        rel: [f32; 3],
        p: [f64; 3],
        light: &dyn Fn(i32, i32, i32) -> (u8, u8),
    ) -> bool {
        let Some(rect) = self.atlas.rect("entity/experience/experience_orb") else { return false };
        let value = e.meta_int(8).unwrap_or(0);
        const STEPS: [i32; 10] = [2477, 1237, 617, 307, 149, 73, 37, 17, 7, 3];
        let icon = STEPS.iter().position(|&s| value >= s).map_or(0, |i| 10 - i as i32);
        let cell = [
            rect[0] + (icon % 4) as f32 * rect[2] / 4.0,
            rect[1] + (icon / 4) as f32 * rect[3] / 4.0,
            rect[2] / 4.0,
            rect[3] / 4.0,
        ];
        let a = e.age as f32 + pt;
        let r = (((a / 2.0).sin() + 1.0) * 0.5 * 255.0).floor() as u32;
        let b = (((a / 2.0 + std::f32::consts::PI * 4.0 / 3.0).sin() + 1.0) * 0.1 * 255.0).floor() as u32;
        let color = r | 255 << 8 | b << 16 | 128 << 24;
        // Light at the eyes' block (the game's light probe), not at the feet.
        let (bl, sl) = self.eye_light(e, p, light);
        let packed = (u32::from((bl + 7).min(15)) * 16) | ((u32::from(sl) * 16) << 8);
        let m = Mat4::from_translation(Vec3::new(rel[0], rel[1] + 0.1, rel[2]))
            * self.cam_rot
            * Mat4::from_scale(Vec3::splat(0.3));
        let base = self.mats.len() as u32;
        self.mats.push(PartMat::new(&m));
        self.instances[1].push((1, 0, 6, Instance { base, rect: cell, color, light: packed, overlay: 10 << 16 }));
        true
    }

    /// Lights of the dimension ("nether" cardinal lighting).
    pub fn set_dimension(&self, queue: &wgpu::Queue, nether: bool) {
        self.nether.set(nether);
        // Only the two lights (the shadow rectangle does not change).
        queue.write_buffer(&self.lights, 0, bytemuck::cast_slice(&world_lights(nether)));
    }

    /// Layers of an entity kind (`entities.md` part 3), resolved once: index in
    /// the arena.
    fn resolve(&mut self, kind_id: i32, kind: &str, baby: bool, skin: u32) -> usize {
        if let Some(&i) = self.resolved.get(&(kind_id, baby, skin)) {
            return i;
        }
        let mut out = Vec::new();
        for (model, texture, pass, wool) in self.layer_names(kind, baby, skin) {
            let (Some(&layer), Some(rect)) = (self.layer_index.get(&model), self.atlas.rect(&texture)) else {
                continue;
            };
            out.push(Resolved { layer, rect, pass, wool });
        }
        let i = self.arena.len();
        self.arena.push(out);
        self.resolved.insert((kind_id, baby, skin), i);
        i
    }

    fn layer_names(&self, k: &str, baby: bool, skin: u32) -> Vec<(String, String, Pass, Wool)> {
        if let Some(v) = animals::layers(self, k, baby, skin)
            .or_else(|| creatures::layers(self, k, baby, skin))
            .or_else(|| dragon::layers(self, k, baby, skin))
            .or_else(|| wither::layers(self, k, baby, skin))
            .or_else(|| shulker::layers(self, k, baby, skin))
            .or_else(|| objects::layers(self, k, baby, skin))
        {
            return v;
        }
        let one = |m: &str, t: &str| vec![(m.to_owned(), t.to_owned(), Pass::Cutout, Wool::None)];
        // Models with culled back faces: arrows, bats, baby turtle.
        let culled = |m: &str, t: &str| vec![(m.to_owned(), t.to_owned(), Pass::CutoutCull, Wool::None)];
        let special = match k {
            "bat" => Some(("minecraft:bat#main", "entity/bat/bat")),
            "turtle" if baby => Some(("minecraft:turtle_baby#main", "entity/turtle/turtle_baby")),
            _ => None,
        };
        if let Some((m, t)) = special
            && self.layer_index.contains_key(m)
        {
            return culled(m, t);
        }
        match k {
            "player" | "mannequin" => {
                let slim = skin < 9;
                let tex = format!("entity/player/{}/{}", if slim { "slim" } else { "wide" }, SKINS[skin as usize % 9]);
                let model = if slim { "minecraft:player_slim#main" } else { "minecraft:player#main" };
                vec![(model.to_owned(), tex, Pass::Translucent, Wool::None)]
            }
            // Arrows (part C § 4.3): `skin` 1 = effect arrow.
            "arrow" => culled(
                "minecraft:arrow#main",
                if skin == 1 { "entity/projectiles/arrow_tipped" } else { "entity/projectiles/arrow" },
            ),
            "spectral_arrow" => culled("minecraft:arrow#main", "entity/projectiles/arrow_spectral"),
            // Humanoids and illagers (`entities-humanoids.md`).
            "husk" | "drowned" => {
                let (m, t) = if baby {
                    (format!("minecraft:{k}_baby#main"), format!("entity/zombie/{k}_baby"))
                } else {
                    (format!("minecraft:{k}#main"), format!("entity/zombie/{k}"))
                };
                let mut v = vec![(m, t, Pass::Cutout, Wool::None)];
                if k == "drowned" {
                    let (m, t) = if baby {
                        ("minecraft:drowned_baby#outer", "entity/zombie/drowned_outer_layer_baby")
                    } else {
                        ("minecraft:drowned#outer", "entity/zombie/drowned_outer_layer")
                    };
                    v.push((m.into(), t.into(), Pass::Cutout, Wool::None));
                }
                v
            }
            "giant" => one("minecraft:giant#main", "entity/zombie/zombie"),
            "zombified_piglin" | "piglin" | "piglin_brute" => {
                // Translucent body; the brute is always adult.
                let b = baby && k != "piglin_brute";
                let m = if b { format!("minecraft:{k}_baby#main") } else { format!("minecraft:{k}#main") };
                let t = if b { format!("entity/piglin/{k}_baby") } else { format!("entity/piglin/{k}") };
                vec![(m, t, Pass::Translucent, Wool::None)]
            }
            "stray" | "bogged" => vec![
                (format!("minecraft:{k}#main"), format!("entity/skeleton/{k}"), Pass::Cutout, Wool::None),
                (format!("minecraft:{k}#outer"), format!("entity/skeleton/{k}_overlay"), Pass::Cutout, Wool::None),
            ],
            "parched" | "wither_skeleton" => one(&format!("minecraft:{k}#main"), &format!("entity/skeleton/{k}")),
            "pillager" | "vindicator" | "evoker" | "illusioner" => {
                one(&format!("minecraft:{k}#main"), &format!("entity/illager/{k}"))
            }
            // `skin` 1: charging vex.
            "vex" => vec![(
                "minecraft:vex#main".into(),
                if skin == 1 { "entity/illager/vex_charging" } else { "entity/illager/vex" }.into(),
                Pass::Translucent,
                Wool::None,
            )],
            "evoker_fangs" => one("minecraft:evoker_fangs#main", "entity/illager/evoker_fangs"),
            "ravager" => one("minecraft:ravager#main", "entity/illager/ravager"),
            "zoglin" | "hoglin" => {
                if baby {
                    one(&format!("minecraft:{k}_baby#main"), &format!("entity/hoglin/{k}_baby"))
                } else {
                    one(&format!("minecraft:{k}#main"), &format!("entity/hoglin/{k}"))
                }
            }
            // `skin` 1: small stand.
            "armor_stand" => one(
                if skin == 1 { "minecraft:armor_stand_small#main" } else { "minecraft:armor_stand#main" },
                "entity/armorstand/armorstand",
            ),
            "zombie" => one(
                if baby { "minecraft:zombie_baby#main" } else { "minecraft:zombie#main" },
                if baby { "entity/zombie/zombie_baby" } else { "entity/zombie/zombie" },
            ),
            "spider" | "cave_spider" => {
                let mut v = one("minecraft:spider#main", &format!("entity/spider/{k}"));
                v.push(("minecraft:spider#main".into(), "entity/spider/spider_eyes".into(), Pass::Eyes, Wool::None));
                v
            }
            "cow" | "pig" | "chicken" | "cat" | "frog" | "wolf" => {
                // Received variant (low 8 bits of `skin`), otherwise the temperate variant; state above.
                let state = skin >> 8;
                let def = self.variant_def(&format!("minecraft:{k}_variant"), (skin & 0xff) as usize);
                let tex = match def {
                    // Tamed (priority) or angry wolf: own texture of the variant.
                    Some(d) if k == "wolf" && state & 3 != 0 => {
                        let (a, b) = if state & 1 != 0 { &d.tame } else { &d.angry };
                        if baby { b.clone() } else { a.clone() }
                    }
                    Some(d) => {
                        if baby {
                            d.baby_asset.clone()
                        } else {
                            d.asset.clone()
                        }
                    }
                    None => format!("entity/{k}/{k}_temperate{}", if baby { "_baby" } else { "" }),
                };
                let kind = def.map_or("normal", |d| d.model.as_str());
                let model = match (k, kind, baby) {
                    ("cow", "warm" | "cold", false) => format!("minecraft:{kind}_cow#main"),
                    ("cow", "warm" | "cold", true) => format!("minecraft:{kind}_cow_baby#main"),
                    ("pig" | "chicken", "cold", false) => format!("minecraft:cold_{k}#main"),
                    (_, _, true) => format!("minecraft:{k}_baby#main"),
                    _ => format!("minecraft:{k}#main"),
                };
                let mut v = one(&model, &tex);
                let b = if baby { "_baby" } else { "" };
                match k {
                    // Wolf armor (adult): "wolf_body" layers of the item, dyeable layer at the item's
                    // tint, cracks; then dyed collar, absent when invisible.
                    "wolf" => {
                        let armor = state >> 7 & 0xfff;
                        if armor != 0 && !baby {
                            if let Some(info) = self.source.items.get(armor as usize - 1)
                                && let Some(defs) =
                                    self.equipment.get(&info.equip_asset).and_then(|t| t.get("wolf_body"))
                            {
                                for (t, dye) in defs {
                                    let wool = dye.map_or(Wool::None, Wool::BodyDye);
                                    v.push((
                                        "minecraft:wolf_armor#main".into(),
                                        format!("entity/equipment/wolf_body/{t}"),
                                        Pass::Cutout,
                                        wool,
                                    ));
                                }
                            }
                            let crack = state >> 19 & 3;
                            if crack != 0 {
                                let lvl = ["low", "medium", "high"][crack as usize - 1];
                                v.push((
                                    "minecraft:wolf_armor#main".into(),
                                    format!("entity/wolf/wolf_armor_crackiness_{lvl}"),
                                    Pass::Translucent,
                                    Wool::None,
                                ));
                            }
                        }
                        let collar = state >> 2 & 31;
                        if collar != 0 {
                            v.push((
                                model.clone(),
                                format!("entity/wolf/wolf_collar{b}"),
                                Pass::Cutout,
                                Wool::Fixed(0xff00_0000 | DYE_COLORS[collar as usize - 1]),
                            ));
                        }
                    }
                    "cat" if state != 0 => v.push((
                        format!("minecraft:cat{b}#collar"),
                        format!("entity/cat/cat_collar{b}"),
                        Pass::Cutout,
                        Wool::Fixed(0xff00_0000 | DYE_COLORS[state as usize - 1]),
                    )),
                    // Saddle (adult only): "pig_saddle" layers of the item.
                    "pig" if !baby => {
                        for t in self.equip_textures(state & 0xfff, "pig_saddle") {
                            v.push(("minecraft:pig#saddle".into(), t, Pass::Cutout, Wool::None));
                        }
                    }
                    _ => {}
                }
                v
            }
            "sheep" => {
                let mut v = one(
                    if baby { "minecraft:sheep_baby#main" } else { "minecraft:sheep#main" },
                    if baby { "entity/sheep/sheep_baby" } else { "entity/sheep/sheep" },
                );
                if !baby {
                    v.push((
                        "minecraft:sheep#wool_undercoat".into(),
                        "entity/sheep/sheep_wool_undercoat".into(),
                        Pass::Cutout,
                        Wool::Undercoat,
                    ));
                }
                v.push((
                    if baby { "minecraft:sheep_baby#wool" } else { "minecraft:sheep#wool" }.into(),
                    if baby { "entity/sheep/sheep_wool_baby" } else { "entity/sheep/sheep_wool" }.into(),
                    Pass::Cutout,
                    Wool::Coat,
                ));
                v
            }
            "villager" | "zombie_villager" => self.villager_layers(k, baby, skin >> 8),
            // Copper golem: texture and eyes ("eyes" pass) depending on oxidation.
            "copper_golem" => {
                let suffix = ["", "_exposed", "_weathered", "_oxidized"][(skin >> 8 & 3) as usize];
                let mut v = one("minecraft:copper_golem#main", &format!("entity/copper_golem/copper_golem{suffix}"));
                v.push((
                    "minecraft:copper_golem#main".into(),
                    format!("entity/copper_golem/copper_golem_eyes{suffix}"),
                    Pass::Eyes,
                    Wool::None,
                ));
                v
            }
            "strider" => {
                let state = skin >> 8;
                let b = if baby { "_baby" } else { "" };
                let cold = if state & (1 << 12) != 0 { "_cold" } else { "" };
                let mut v = one(&format!("minecraft:strider{b}#main"), &format!("entity/strider/strider{cold}{b}"));
                if !baby {
                    for t in self.equip_textures(state & 0xfff, "strider_saddle") {
                        v.push(("minecraft:strider#saddle".into(), t, Pass::Cutout, Wool::None));
                    }
                }
                v
            }
            "sniffer" => {
                let (m, t) = sniffer_layer(baby);
                one(m, t)
            }
            _ => {
                let model = format!("minecraft:{k}{}#main", if baby { "_baby" } else { "" });
                let tex = [format!("entity/{k}/{k}"), format!("entity/{k}"), format!("entity/{k}/{k}_temperate")]
                    .into_iter()
                    .find(|t| self.atlas.contains(t));
                let mut v = match tex {
                    Some(t) if self.layer_index.contains_key(&model) => {
                        vec![(model.clone(), t, default_body_pass(k), Wool::None)]
                    }
                    _ => Vec::new(),
                };
                let state = skin >> 8;
                match k {
                    // Glowing eyes ("eyes" pass, drawn even when invisible).
                    "enderman" | "phantom" => {
                        v.push((model.clone(), format!("entity/{k}/{k}_eyes"), Pass::Eyes, Wool::None));
                    }
                    // Charged creeper: swirling halo on the inflated model.
                    "creeper" if state & 1 != 0 => {
                        v.push((
                            "minecraft:creeper#armor".into(),
                            "entity/creeper/creeper_armor".into(),
                            Pass::Swirl,
                            Wool::None,
                        ));
                    }
                    // Warden: bioluminescence, pulsing spots, tentacles, heart (emissive,
                    // absent when invisible).
                    "warden" => {
                        for (part, tex, id) in [
                            ("bioluminescent", "warden_bioluminescent_layer", 0),
                            ("pulsating_spots", "warden_pulsating_spots_1", 1),
                            ("pulsating_spots", "warden_pulsating_spots_2", 2),
                            ("tendrils", "warden", 3),
                            ("heart", "warden_heart", 4),
                        ] {
                            v.push((
                                format!("minecraft:warden#{part}"),
                                format!("entity/warden/{tex}"),
                                Pass::Emissive,
                                Wool::Pulse(id),
                            ));
                        }
                    }
                    // Eyes of the active creaking ("eyes" pass, even when invisible).
                    "creaking" => v.push((
                        "minecraft:creaking#eyes".into(),
                        "entity/creaking/creaking_eyes".into(),
                        Pass::Eyes,
                        Wool::Pulse(5),
                    )),
                    // Breeze: scrolling wind then emissive eyes.
                    "breeze" => {
                        v.push((
                            "minecraft:breeze#wind".into(),
                            "entity/breeze/breeze_wind".into(),
                            Pass::Wind,
                            Wool::None,
                        ));
                        v.push((
                            "minecraft:breeze#eyes".into(),
                            "entity/breeze/breeze_eyes".into(),
                            Pass::Emissive,
                            Wool::None,
                        ));
                    }
                    // Iron golem cracks (low, medium, high), absent when invisible.
                    "iron_golem" if state != 0 => {
                        let lvl = ["low", "medium", "high"][(state as usize - 1).min(2)];
                        v.push((
                            model.clone(),
                            format!("entity/iron_golem/iron_golem_crackiness_{lvl}"),
                            Pass::Cutout,
                            Wool::None,
                        ));
                    }
                    _ => {}
                }
                v
            }
        }
    }

    /// Equipment textures (`entity/equipment/<type>/<texture>`) of item `key` (id
    /// + 1, 0 = nothing) for a layer type.
    fn equip_textures(&self, key: u32, ty: &str) -> Vec<String> {
        let Some(info) = key.checked_sub(1).and_then(|i| self.source.items.get(i as usize)) else { return Vec::new() };
        self.equipment
            .get(&info.equip_asset)
            .and_then(|t| t.get(ty))
            .map(|l| l.iter().map(|(t, _)| format!("entity/equipment/{ty}/{t}")).collect())
            .unwrap_or_default()
    }

    /// Villager and zombie villager: body, then type (biome; baby: "baby" texture), profession
    /// and level (adults, profession ≠ none; level except for the nitwit). The type's hat is
    /// removed (hatless model) if the profession carries a full one, or a partial one against a type with a
    /// full hat. `state`: type, profession (6 bits each), level.
    fn villager_layers(&self, k: &str, baby: bool, state: u32) -> Vec<(String, String, Pass, Wool)> {
        let b = if baby { "_baby" } else { "" };
        let model = format!("minecraft:{k}{b}#main");
        let mut v = vec![(model.clone(), format!("entity/{k}/{k}{b}"), Pass::Cutout, Wool::None)];
        let name = |reg: &str, id: u32| -> Option<String> {
            let e = self.source.registry(reg)?.entries.get(id as usize)?;
            Some(e.strip_prefix("minecraft:").unwrap_or(e).to_owned())
        };
        let (Some(ty), Some(prof)) =
            (name("minecraft:villager_type", state & 63), name("minecraft:villager_profession", state >> 6 & 63))
        else {
            return v;
        };
        let hat = |kind: &str, n: &str| self.villager_hats.get(&format!("entity/{k}/{kind}/{n}")).copied().unwrap_or(0);
        let (type_hat, prof_hat) = (hat("type", &ty), hat("profession", &prof));
        let type_hat_visible = type_hat_visible(type_hat, prof_hat);
        let type_model = if type_hat_visible { model.clone() } else { format!("minecraft:{k}{b}_no_hat#main") };
        let type_dir = if baby { "baby" } else { "type" };
        v.push((type_model, format!("entity/{k}/{type_dir}/{ty}"), Pass::Cutout, Wool::None));
        if prof != "none" && !baby {
            v.push((model.clone(), format!("entity/{k}/profession/{prof}"), Pass::Cutout, Wool::None));
            if prof != "nitwit" {
                const LEVELS: [&str; 5] = ["stone", "iron", "gold", "emerald", "diamond"];
                let lvl = LEVELS[((state >> 12 & 7).clamp(1, 5) - 1) as usize];
                v.push((model, format!("entity/{k}/profession_level/{lvl}"), Pass::Cutout, Wool::None));
            }
        }
        v
    }

    /// Copies as a flat colour the instances (cutout and translucent) placed since `marks` for
    /// the glowing entity that has just been processed.
    fn collect_glow(&mut self, glowing: Option<Glowing>) {
        let Some((rgb, marks, only)) = glowing else { return };
        // ABGR colour of the instance (R in the low byte), opaque alpha.
        let color = (rgb >> 16 & 0xff) | (rgb & 0xff00) | (rgb & 0xff) << 16 | 0xff00_0000;
        for (k, &from) in marks.iter().enumerate() {
            let list = &self.instances[GLOW_PASSES[k]];
            let from = from.min(list.len());
            for &(src, first, count, inst) in &list[from..] {
                self.glow_list.push((src, first, count, Instance { color, ..inst }));
            }
        }
        // Invisible body of a glowing entity: silhouette only.
        for &(src, first, count, inst) in &self.glow_only[only.min(self.glow_only.len())..] {
            self.glow_list.push((src, first, count, Instance { color, ..inst }));
        }
    }

    /// Silhouettes are to be drawn (visible glowing entities).
    pub fn has_glow(&self) -> bool {
        !self.glow_batches.is_empty()
    }

    /// Coloured silhouettes of the glowing entities (group 0 = camera already bound), towards the
    /// outline target.
    pub fn draw_glow(&self, pass: &mut wgpu::RenderPass<'_>) {
        let (Some(bind), Some((inst, _))) = (&self.bind, &self.inst_buf) else { return };
        if self.glow_batches.is_empty() {
            return;
        }
        pass.set_pipeline(&self.outline);
        pass.set_bind_group(1, bind, &[]);
        pass.set_vertex_buffer(1, inst.slice(..));
        let mut bound = u8::MAX;
        for (src, verts, first, count) in &self.glow_batches {
            if *src != bound {
                match src {
                    0 => pass.set_vertex_buffer(0, self.statics.slice(..)),
                    _ => match &self.item_buf {
                        Some((b, _)) => pass.set_vertex_buffer(0, b.slice(..)),
                        None => continue,
                    },
                }
                bound = *src;
            }
            pass.draw(verts.clone(), *first..*first + *count);
        }
    }

    /// The entity is within its display distance from camera `cam` (same test as the render).
    pub fn within_view_distance(&self, e: &Entity, pt: f32, cam: [f64; 3]) -> bool {
        let p = e.render_pos(pt);
        let d2 = (0..3).map(|i| (p[i] - cam[i]).powi(2)).sum::<f64>();
        within_render_distance(e, d2, self.view_scale, &self.dims)
    }

    /// Prepares the instances of all visible entities. `origin` = integer origin of the
    /// camera (x, z); `light(x, y, z)` = (block, sky); `visible(min, max)` = box within the field of view.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare<'e>(
        &mut self,
        entities: impl Iterator<Item = &'e Entity>,
        pt: f32,
        origin: [i32; 2],
        cam: [f64; 3],
        light: &dyn Fn(i32, i32, i32) -> (u8, u8),
        visible: &dyn Fn([f32; 3], [f32; 3]) -> bool,
        items: Option<&rc_assets::item::ItemModels>,
        world_q: &ShadowWorld<'_>,
    ) {
        self.mats.clear();
        self.shadow_verts.clear();
        self.objects.portals.clear();
        let eye =
            Vec3::new((cam[0] - f64::from(origin[0])) as f32, cam[1] as f32, (cam[2] - f64::from(origin[1])) as f32);
        self.layering = crate::render::view_layering(eye, 1.0);
        self.layering_forward = crate::render::view_layering(eye, -1.0);
        self.eye = eye;
        for v in &mut self.instances {
            v.clear();
        }
        self.set_current(None, None);
        let opt = self.opt;
        // From farthest to nearest (translucent order). Sizes (base, full)
        // computed once per entity (`SIZEONCE`); kind flags (`KINDINFO`).
        let mut list: Vec<(&Entity, f64, Option<Sizes>, KindInfo)> = entities
            .map(|e| {
                let p = e.render_pos(pt);
                let sizes = opt.size_once.then(|| {
                    let b = e.base_size(&self.dims);
                    (b, e.size_from(b))
                });
                let info = if opt.kind_info { self.kind_of(e) } else { KindInfo::default() };
                (e, (0..3).map(|i| (p[i] - cam[i]).powi(2)).sum::<f64>(), sizes, info)
            })
            .filter(|&(e, d, sizes, _)| match sizes {
                Some((_, full)) => within_render_distance_of(e, d, self.view_scale, || full),
                None => within_render_distance(e, d, self.view_scale, &self.dims),
            })
            .collect();
        list.sort_by(|a, b| b.1.total_cmp(&a.1));
        for v in &mut self.free_verts {
            v.clear();
        }
        self.crystals.clear();
        let is = |e: &Entity, ki: KindInfo, f: u32, k: &str| if opt.kind_info { ki.has(f) } else { e.kind == k };
        if list.iter().any(|&(e, _, _, ki)| is(e, ki, K_ENDER_DRAGON, "ender_dragon")) {
            self.crystals.extend(
                list.iter()
                    .filter(|&&(e, _, _, ki)| is(e, ki, K_END_CRYSTAL, "end_crystal"))
                    .map(|(e, ..)| (e.render_pos(pt), objects::crystal_time(e, pt))),
            );
        }
        let mut drawn = 0;
        self.glow_list.clear();
        // Current glowing entity: colour and end of the instances already placed per pass.
        self.glow_only.clear();
        let mut glowing: Option<Glowing> = None;
        let new = opt.kind_info;
        for (e, _, sizes, ki) in list {
            self.collect_glow(glowing.take());
            self.set_current(Some(e), sizes);
            let mut p = e.render_pos(pt);
            // Shulker slide after a teleport (render offset).
            if let Some(o) = shulker::render_offset(e, pt) {
                p = [p[0] + o[0], p[1] + o[1], p[2] + o[2]];
            }
            // Shaking of the creepy enderman.
            if let Some(o) = creatures::render_offset(e) {
                p = [p[0] + o[0], p[1] + o[1], p[2] + o[2]];
            }
            // Crouching avatar: model (and flames) lowered by 2/16 × scale; the shadow and the
            // light probes (`p`) keep the real position.
            let rel = [
                (p[0] - f64::from(origin[0])) as f32,
                (p[1] + crouch_offset(e)) as f32,
                (p[2] - f64::from(origin[1])) as f32,
            ];
            // Wide visibility box (collision box inflated by 0.5 in the game); large mobs separate.
            let (hw, hh) = if new {
                if ki.has(K_BIG_BOX) {
                    (10.0, 14.0)
                } else if ki.has(K_ILLUSIONER) {
                    (5.5, 5.0)
                } else {
                    (2.5, 5.0)
                }
            } else {
                match e.kind.as_str() {
                    "ender_dragon" | "ghast" | "happy_ghast" | "giant" | "wither" => (10.0, 14.0),
                    // Illusioner: box widened by 3 in x and z (illusion copies).
                    "illusioner" => (5.5, 5.0),
                    _ => (2.5, 5.0),
                }
            };
            // Enlarged living entity: the box grows with the scale (never shrunk, wide margin).
            let grow = if e.class != Kind::Other { living_scale(e).max(1.0) } else { 1.0 };
            let (hw, hh) = (hw * grow, hh * grow);
            // Lightning, bobber, dragon: never culled. Targeted crystal: its beam may be
            // visible alone.
            // Displays: never culled while width or height is 0 (default); otherwise
            // real box inflated by 0.5.
            let (cull, always) = if new {
                let is_display = ki.has(K_DISPLAY);
                // Box specific to displays only (`display_cull_box` returns `None` otherwise).
                let cull = if is_display { display_cull_box(e) } else { None };
                let always = ki.has(K_ALWAYS)
                    || (ki.has(K_END_CRYSTAL) && objects::beam_target(e).is_some())
                    || (is_display && cull.is_none());
                (cull, always)
            } else {
                let is_display = matches!(e.kind.as_str(), "block_display" | "item_display" | "text_display");
                let cull = display_cull_box(e);
                let always = matches!(e.kind.as_str(), "lightning_bolt" | "fishing_bobber" | "ender_dragon")
                    || (e.kind == "end_crystal" && objects::beam_target(e).is_some())
                    || (is_display && cull.is_none());
                (cull, always)
            };
            let (hw, hh) = cull.map_or((hw, hh), |(w, h)| (w * 0.5 + 0.5, h + 0.5));
            if !(always || visible([rel[0] - hw, rel[1] - 0.5, rel[2] - hw], [rel[0] + hw, rel[1] + hh, rel[2] + hw])) {
                continue;
            }
            // Invisible: never a shadow; the body of a living entity is handled by `push_entity`
            // (layers, armor and held items remain), the frame by its own render; the other
            // entities ignore the flag.
            if e.flags() & 0x20 == 0 {
                self.push_shadow(e, pt, origin, cam, world_q);
            }
            self.push_flame(e, new.then_some(ki), rel, p, light);
            if let Some(&rgb) = self.glow.get(&e.uuid) {
                glowing =
                    Some((rgb, std::array::from_fn(|i| self.instances[GLOW_PASSES[i]].len()), self.glow_only.len()));
            }
            if let Some(d) = self.push_object(e, pt, rel, p, cam, light, items, world_q) {
                drawn += usize::from(d);
                continue;
            }
            if is(e, ki, K_PAINTING, "painting") {
                drawn += usize::from(self.push_painting(e, rel, p, light));
                continue;
            }
            if is(e, ki, K_ORB, "experience_orb") {
                drawn += usize::from(self.push_orb(e, pt, rel, p, light));
                continue;
            }
            if is(e, ki, K_ITEM, "item") {
                if let Some(models) = items {
                    drawn += usize::from(self.push_item(e, models, pt, rel, p, light));
                }
                continue;
            }
            // Family whose skin function answers (each by kind alone).
            let family = if new {
                match ki.family {
                    1 => animals::skin(self, e),
                    2 => creatures::skin(self, e),
                    3 => dragon::skin(self, e),
                    4 => wither::skin(self, e),
                    5 => shulker::skin(self, e),
                    _ => None,
                }
            } else {
                animals::skin(self, e)
                    .or_else(|| creatures::skin(self, e))
                    .or_else(|| dragon::skin(self, e))
                    .or_else(|| wither::skin(self, e))
                    .or_else(|| shulker::skin(self, e))
            };
            let skin = match e.kind.as_str() {
                _ if family.is_some() => family.unwrap_or_default(),
                "player" | "mannequin" => u32::from(avatar_default_skin(e)),
                "vex" => u32::from(e.meta_byte(16) & 1 != 0),
                "armor_stand" => u32::from(e.meta_byte(15) & 1 != 0),
                // Effect colour (index 11) > 0: effect arrow.
                "arrow" => u32::from(e.meta_int(11).is_some_and(|c| c > 0)),
                _ => self.own_skin(e, new.then_some(ki)),
            };
            let layers = match self.custom_skins.get(&e.uuid) {
                Some(&i) if e.kind == "player" => i,
                _ => {
                    let base = self.resolve(e.kind_id, &e.kind, e.baby(), skin);
                    if e.kind == "mannequin" { self.mannequin_layers(e, base) } else { base }
                }
            };
            if self.arena[layers].is_empty() {
                continue;
            }
            drawn += 1;
            let env = self.pose_env(e, pt, world_q);
            if e.kind == "illusioner" && e.flags() & 0x20 != 0 {
                // Invisible illusioner: four visible offset copies, which wave.
                let age = e.age as f32 + pt;
                for (i, o) in illusion_offsets(e, pt).into_iter().enumerate() {
                    let w = illusion_wobble(i, age);
                    let at = std::array::from_fn(|k| rel[k] + (o[k] + w[k]) as f32);
                    self.push_entity(e, layers, pt, at, p, light, items, env);
                }
            } else {
                self.push_entity(e, layers, pt, rel, p, light, items, env);
            }
        }
        self.set_current(None, None);
        self.collect_glow(glowing.take());
        self.drawn = drawn;
        self.hand_from = std::array::from_fn(|i| self.instances[i].len());
        self.gui_from = None;
        self.totem_from = None;
    }

    /// Key of the layers of a kind handled here: variant (low 8 bits, 255 unknown) for the
    /// animals with variants, then the state that changes the layers (`state_bits`, shifted by 8).
    fn own_skin(&self, e: &Entity, ki: Option<KindInfo>) -> u32 {
        let ser = ki.map_or_else(|| own_variant_ser(&e.kind), |k| k.variant_ser);
        let variant = if ser == 0 { 0 } else { e.variant(ser).map_or(255, |v| v.clamp(0, 254) as u32) };
        variant | self.state_bits(e) << 8
    }

    /// State that changes the layers: wolf (tamed, angry, collar colour + 1), cat
    /// (collar + 1 if tamed), pig and strider (saddle: item + 1; frozen strider), charged
    /// creeper, iron golem cracks, snow golem pumpkin, villager (type,
    /// profession, level).
    fn state_bits(&self, e: &Entity) -> u32 {
        let collar = |i: u8| (e.meta_int(i).unwrap_or(14).clamp(0, 15) + 1) as u32;
        let saddle = || e.equipment[7].as_ref().and_then(|s| u32::try_from(s.item + 1).ok()).filter(|&i| i < 4096);
        match e.kind.as_str() {
            "wolf" => {
                let tame = e.meta_byte(18) & 4 != 0;
                let base = if tame { 1 | collar(21) << 2 } else { u32::from(animals::angry(e, 22)) << 1 };
                // Wolf armor (body): item + 1 and cracks depending on wear (0.95 / 0.69 / 0.32).
                let armor = e.equipment[6].as_ref().map_or(0, |s| {
                    let key = u32::try_from(s.item + 1).ok().filter(|&i| i < 4096).unwrap_or(0);
                    let info = usize::try_from(s.item).ok().and_then(|i| self.source.items.get(i));
                    let view = crate::stack::StackRef {
                        stack: s,
                        info,
                        ids: &self.ids,
                        data: &self.source,
                        hold: crate::stack::Hold::NONE,
                    };
                    let max = rc_assets::item::StackView::max_damage(&view);
                    let crack = if max > 0 {
                        let f = (max - rc_assets::item::StackView::damage(&view)) as f32 / max as f32;
                        if f < 0.32 {
                            3
                        } else if f < 0.69 {
                            2
                        } else {
                            u32::from(f < 0.95)
                        }
                    } else {
                        0
                    };
                    key | crack << 12
                });
                base | armor << 7
            }
            "cat" if e.meta_byte(18) & 4 != 0 => collar(23),
            "pig" => saddle().unwrap_or(0),
            "strider" => saddle().unwrap_or(0) | u32::from(e.meta_bool(19)) << 12,
            "creeper" => u32::from(e.meta_bool(17)),
            "iron_golem" => {
                let health = match e.meta.get(&9) {
                    Some(rc_proto_v777::entity::Meta::Float(h)) => *h / 100.0,
                    _ => 1.0,
                };
                // Cracks: below 3/4, 1/2, 1/4 of the maximum health (100).
                if health < 0.25 {
                    3
                } else if health < 0.5 {
                    2
                } else {
                    u32::from(health < 0.75)
                }
            }
            "snow_golem" => u32::from(e.meta_byte(16) & 0x10 != 0),
            // Copper golem oxidation (0 to 3).
            "copper_golem" => match e.meta.get(&16) {
                Some(rc_proto_v777::entity::Meta::Variant(v, _) | rc_proto_v777::entity::Meta::Int(v)) => {
                    (*v).clamp(0, 3) as u32
                }
                _ => 0,
            },
            "villager" | "zombie_villager" => {
                let i = if e.kind == "villager" { 19 } else { 20 };
                match e.meta.get(&i) {
                    Some(rc_proto_v777::entity::Meta::Villager([t, p, l])) => {
                        (*t.clamp(&0, &63) as u32) | (*p.clamp(&0, &63) as u32) << 6 | (*l.clamp(&1, &5) as u32) << 12
                    }
                    // Data left at their starting value (not sent): plains, no
                    // profession, level 1.
                    _ => {
                        let id = |reg: &str, key: &str| {
                            self.source.registry(reg).and_then(|r| r.entries.iter().position(|e| e == key)).unwrap_or(0)
                                as u32
                        };
                        id("minecraft:villager_type", "minecraft:plains")
                            | id("minecraft:villager_profession", "minecraft:none") << 6
                            | 1 << 12
                    }
                }
            }
            _ => 0,
        }
    }

    /// Slot `slot` carries the item `minecraft:<name>`.
    fn holds(&self, e: &Entity, slot: usize, name: &str) -> bool {
        let item = e.equipment[slot].as_ref().and_then(|s| usize::try_from(s.item).ok());
        let bit = if self.opt.item_flags { item_bit_of(name) } else { 0 };
        if bit != 0 {
            return item.and_then(|i| self.item_bits.get(i)).is_some_and(|b| b & bit != 0);
        }
        item.and_then(|i| self.item_names()?.get(i)).is_some_and(|n| n.strip_prefix("minecraft:") == Some(name))
    }

    /// Arm pose of a piglin or brute (outside dancing), from its equipment.
    fn piglin_pose(&self, e: &Entity) -> u8 {
        let main = e.equipment[0].as_ref();
        // Melee weapon: the main stack carries the "tool" component (default or added).
        let tool = main.is_some_and(|s| {
            let id = self.ids.of("minecraft:tool");
            let info = usize::try_from(s.item).ok().and_then(|i| self.source.items.get(i));
            !id.is_some_and(|i| s.removed.contains(&i))
                && (id.is_some_and(|i| s.component(i).is_some()) || info.is_some_and(|i| i.tool.is_some()))
        });
        let melee = e.meta_byte(15) & 4 != 0 && tool;
        if e.kind == "piglin_brute" {
            return if melee { piglin_pose::MELEE } else { piglin_pose::DEFAULT };
        }
        let loved = e.equipment[1]
            .as_ref()
            .is_some_and(|s| crate::menu_special::in_tag("minecraft:piglin_loved", s.item) == Some(true));
        // Crossbow held (in either hand) but charge read from the main hand, like the game.
        let charged = main.is_some_and(|s| {
            let info = usize::try_from(s.item).ok().and_then(|i| self.source.items.get(i));
            let view = crate::stack::StackRef {
                stack: s,
                info,
                ids: &self.ids,
                data: &self.source,
                hold: crate::stack::Hold::NONE,
            };
            rc_assets::item::StackView::charge_type(&view) != "none"
        });
        let crossbow = self.holds(e, 0, "crossbow") || self.holds(e, 1, "crossbow");
        piglin_pose_of(loved, melee, e.meta_bool(18), crossbow && charged)
    }

    /// Context of the poses (bed of a sleeper, water of a swimming player), read from the world.
    fn pose_env(&self, e: &Entity, pt: f32, w: &ShadowWorld<'_>) -> PoseEnv {
        let mut env = PoseEnv::default();
        if e.class == Kind::Other {
            return env;
        }
        if e.pose() == 2
            && let Some(rc_proto_v777::entity::Meta::BlockPos(Some(b))) = e.meta.get(&14)
        {
            let s = (w.state)(b[0], b[1], b[2]);
            let is_bed = self.source.block_of(s).is_some_and(|i| i.name.ends_with("_bed"));
            if is_bed {
                env.bed = self.source.state_properties(s).find(|(k, _)| *k == "facing").and_then(|(_, v)| match v {
                    "south" => Some((0.0, 1.0, 90.0)),
                    "west" => Some((-1.0, 0.0, 0.0)),
                    "north" => Some((0.0, -1.0, 270.0)),
                    "east" => Some((1.0, 0.0, 180.0)),
                    _ => None,
                });
            }
        }
        if matches!(e.kind.as_str(), "player" | "mannequin") && e.pose_anim.swim(pt) > 0.0 {
            env.in_water = self.touches_water(e, w);
        }
        env
    }

    /// Entity box (shrunk by 0.001) in contact with water: water block whose
    /// surface (full under water, otherwise level / 9) reaches the bottom of the box.
    fn touches_water(&self, e: &Entity, w: &ShadowWorld<'_>) -> bool {
        let mask = if self.opt.equip_flag { self.water_bits } else { water_fluids(&self.source) };
        touches_water(&self.source, &self.dims, e, w.state, mask)
    }

    /// Shaking: full freeze (value 7 ≥ 140); conversions (zombie and drowned 18, zombie
    /// villager 19, skeleton 16); piglins and hoglins outside the Nether if they are neither immune
    /// nor without AI; suffocating strider (19).
    fn shaking(&self, e: &Entity) -> bool {
        let frozen = e.meta_int(7).unwrap_or(0) >= 140;
        let zombify = |immune: u8| !e.meta_bool(immune) && e.meta_byte(15) & 1 == 0 && !self.nether.get();
        match e.kind.as_str() {
            "zombie" | "husk" | "drowned" => frozen || e.meta_bool(18),
            "zombie_villager" => frozen || e.meta_bool(19),
            "skeleton" => e.meta_bool(16),
            "stray" | "bogged" | "parched" | "wither_skeleton" => frozen,
            "piglin" | "piglin_brute" => frozen || zombify(16),
            "hoglin" => frozen || zombify(18),
            "strider" => frozen || e.meta_bool(19),
            _ => frozen,
        }
    }

    /// Upside down ("Dinnerbone", "Grumm"): custom name of a living entity; avatar: only
    /// if its cape is shown, player from the name of its profile.
    fn upside_down(&self, e: &Entity) -> bool {
        match e.kind.as_str() {
            "player" => e.meta_byte(16) & 1 != 0 && self.upside_down.contains(&e.uuid),
            "mannequin" if e.meta_byte(16) & 1 == 0 => false,
            _ => custom_name(e).is_some_and(|s| s == "Dinnerbone" || s == "Grumm"),
        }
    }

    /// "Spear" arm poses of a humanoid mob (right, left): lunge in progress of
    /// this arm or held spear; `[0, 0]` if no spear.
    fn mob_spear_poses(&self, e: &Entity) -> [u8; 2] {
        let main_right = self.right_handed(e);
        let spear = |hand: usize| -> bool {
            e.swing.is_some_and(|s| usize::try_from(s.0) == Ok(hand) && s.1 == 2)
                || e.equipment[hand].as_ref().is_some_and(|st| {
                    let i = usize::try_from(st.item).ok();
                    if self.opt.item_flags {
                        i.and_then(|i| self.item_bits.get(i)).is_some_and(|b| b & I_SPEAR != 0)
                    } else {
                        i.and_then(|i| self.item_names()?.get(i)).is_some_and(|n| n.ends_with("_spear"))
                    }
                })
        };
        let (r, l) = (spear(usize::from(!main_right)), spear(usize::from(main_right)));
        if r || l { [if r { arm::SPEAR } else { arm::EMPTY }, if l { arm::SPEAR } else { arm::EMPTY }] } else { [0, 0] }
    }

    /// Arm poses of a player (right, left) from the held items and the current use
    /// (living flags, index 8); the arm posed first follows the official game.
    fn player_arm_poses(&self, e: &Entity, swinging: bool) -> ([u8; 2], bool) {
        use rc_assets::item::StackView as _;
        let right_handed = e.meta_int(15).is_none_or(|v| v != 0);
        let flags = e.meta_byte(8);
        let (using, using_hand) = (flags & 1 != 0, usize::from(flags & 2 != 0));
        let source = &self.source;
        let pose_of = |hand: usize| -> u8 {
            let Some(stack) = &e.equipment[hand] else { return arm::EMPTY };
            let info = usize::try_from(stack.item).ok().and_then(|i| source.items.get(i));
            let view =
                crate::stack::StackRef { stack, info, ids: &self.ids, data: source, hold: crate::stack::Hold::NONE };
            // Short name borrowed from the view (`ITEMFLAGS`), otherwise copied as before.
            let owned;
            let name: &str = if self.opt.item_flags {
                view.item_name().strip_prefix("minecraft:").unwrap_or("")
            } else {
                owned = view.item_name().strip_prefix("minecraft:").unwrap_or("").to_owned();
                &owned
            };
            if !swinging && name == "crossbow" && view.charge_type() != "none" {
                return arm::HOLD;
            }
            if using && using_hand == hand {
                match name {
                    "shield" => return arm::BLOCK,
                    "bow" => return arm::BOW,
                    "trident" => return arm::TRIDENT,
                    "crossbow" => return arm::CHARGE,
                    "spyglass" => return arm::SPYGLASS,
                    "goat_horn" => return arm::HORN,
                    "brush" => return arm::BRUSH,
                    _ => {}
                }
                // "Spear" use animation (kinetic weapon).
                if crate::hand::kinetic_of(&view, &self.ids).is_some() {
                    return arm::SPEAR;
                }
            }
            // "Spear" pose: lunge in progress of this hand, or held spear (`spears`
            // tag of the base game).
            if e.swing.is_some_and(|s| usize::try_from(s.0) == Ok(hand) && s.1 == 2) || name.ends_with("_spear") {
                return arm::SPEAR;
            }
            arm::ITEM
        };
        let main = pose_of(0);
        let mut off = pose_of(1);
        if arm::two_handed(main) {
            off = if e.equipment[1].is_none() { arm::EMPTY } else { arm::ITEM };
        }
        let (r, l) = if right_handed { (main, off) } else { (off, main) };
        // Arm of the current use (hand used), otherwise the off hand except two-handed pose.
        let used_right = (using_hand == 0) == right_handed;
        let off_right = !right_handed;
        let first_right = if using {
            used_right
        } else if arm::two_handed(if off_right { r } else { l }) {
            right_handed
        } else {
            off_right
        };
        ([r, l], first_right)
    }

    #[allow(clippy::too_many_arguments)]
    fn push_entity(
        &mut self,
        e: &Entity,
        layers: usize,
        pt: f32,
        rel: [f32; 3],
        world: [f64; 3],
        light: &dyn Fn(i32, i32, i32) -> (u8, u8),
        items: Option<&rc_assets::item::ItemModels>,
        env: PoseEnv,
    ) {
        let living = e.class != Kind::Other;
        // Kind flags (`KINDINFO`); `kind(f, old)`: flag, otherwise old predicate.
        let new = self.opt.kind_info;
        let ki = if new { self.kind_of(e) } else { KindInfo::default() };
        let kind = |f: u32, old: &dyn Fn() -> bool| if new { ki.has(f) } else { old() };
        let avatar = kind(K_AVATAR, &|| matches!(e.kind.as_str(), "player" | "mannequin"));
        let body = rot_lerp(pt, e.body_o, e.body);
        let head = rot_lerp(pt, e.head_o, e.head);
        let mut yaw = deg(rot_lerp(1.0, 0.0, head - body));
        let mut pitch = deg(e.pitch_o + pt * (e.pitch - e.pitch_o));
        // Upside down: pitch and relative head inverted.
        let upside_down = living && self.upside_down(e);
        if upside_down {
            (yaw, pitch) = (-yaw, -pitch);
        }
        let riding = e.vehicle.is_some();
        let (walk_pos, walk_speed) = if riding { (0.0, 0.0) } else { e.walk_at(pt) };
        let fall_flying = living && e.flags() & (0x80u8 as i8) != 0;
        let speed = if fall_flying {
            let v = e.velocity;
            let s = ((v[0] * v[0] + v[1] * v[1] + v[2] * v[2]) as f32) / 0.2;
            (s * s * s).max(1.0)
        } else {
            1.0
        };
        let state = AnimState {
            swim: e.pose_anim.swim(pt),
            fall_flying,
            speed,
            using: e.meta_byte(8) & 1 != 0,
            use_ticks: e.pose_anim.use_ticks(pt),
            crossbow_charge: {
                let hand = usize::from(e.meta_byte(8) & 2 != 0);
                e.equipment[hand].as_ref().map_or(25, |s| self.crossbow_charge(s)) as f32
            },
            spear: [self.arm_kinetic(e, true), self.arm_kinetic(e, false)],
            use_right: (e.meta_byte(8) & 2 == 0) == self.right_handed(e),
            walk_pos,
            walk_speed,
            age: e.age as f32 + pt,
            yaw,
            pitch,
            swing: e.swing_at(pt),
            swing_right: e.swing.is_none_or(|s| (s.0 == 0) == e.meta_int(15).is_none_or(|v| v != 0)),
            swing_on: e.swing.is_some(),
            swing_kind: e.swing.map_or(1, |s| s.1),
            crouching: e.pose() == 5,
            baby: e.baby(),
            riding,
            arm_pose: [0, 0],
            arm_first_right: true,
            crossbow_held: self.holds(e, 0, "crossbow") || self.holds(e, 1, "crossbow"),
            in_water: e.in_water,
            piglin: if kind(K_PIGLIN, &|| matches!(e.kind.as_str(), "piglin" | "piglin_brute")) {
                self.piglin_pose(e)
            } else {
                0
            },
        };
        // Humanoid mobs: "spear" pose of an arm (lunge in progress or held spear).
        let mob_spear = if kind(K_SPEAR_MOB, &|| {
            matches!(e.kind.as_str(), "zombie" | "husk" | "zombie_villager" | "drowned" | "zombified_piglin")
                || is_skeleton(&e.kind)
        }) {
            self.mob_spear_poses(e)
        } else {
            [0, 0]
        };
        let merge = |mut pose: [u8; 2]| {
            for (p, sp) in pose.iter_mut().zip(mob_spear) {
                if sp == arm::SPEAR && *p <= arm::EMPTY {
                    *p = arm::SPEAR;
                } else if sp != 0 && *p == 0 {
                    *p = arm::EMPTY;
                }
            }
            pose
        };
        let state = if avatar {
            let (arm_pose, arm_first_right) = self.player_arm_poses(e, state.swing > 0.0);
            AnimState { arm_pose, arm_first_right, ..state }
        } else if kind(K_DROWNED, &|| e.kind == "drowned") && e.meta_byte(15) & 4 != 0 && self.holds(e, 0, "trident") {
            // Aggressive drowned with trident: main arm raised ("trident" pose), the other empty.
            let right = e.meta_byte(15) & 2 == 0;
            let arm_pose = if right { [arm::TRIDENT, arm::EMPTY] } else { [arm::EMPTY, arm::TRIDENT] };
            AnimState { arm_pose: merge(arm_pose), arm_first_right: !right, ..state }
        } else if kind(K_SKELETON, &|| is_skeleton(&e.kind)) && e.meta_byte(15) & 4 != 0 && self.holds(e, 0, "bow") {
            // Aggressive skeleton with bow: "bow" pose of the main arm; posed first, the arm
            // of the current use, otherwise that of the off hand.
            let right = e.meta_byte(15) & 2 == 0;
            let arm_pose = if right { [arm::BOW, arm::EMPTY] } else { [arm::EMPTY, arm::BOW] };
            let first_right = if state.using { right != (e.meta_byte(8) & 2 != 0) } else { !right };
            AnimState { arm_pose: merge(arm_pose), arm_first_right: first_right, ..state }
        } else if mob_spear != [0, 0] {
            AnimState { arm_pose: merge([0, 0]), ..state }
        } else {
            state
        };
        // Light at the eyes' block, block at 15 if the entity is burning.
        let (b, s) = self.eye_light(e, world, light);
        // Glow squid: 15 falling back to 0 over the last 10 ticks of darkness (index 17).
        let glow_squid = || {
            let dark = e.meta_int(17).unwrap_or(0);
            let g = ((1.0 - dark as f32 / 10.0).clamp(0.0, 1.0) * 15.0) as u8;
            if g == 15 { 15 } else { g.max(b) }
        };
        let block = if new {
            if e.flags() & 1 != 0 || ki.has(K_SELF_LIT) {
                15
            } else if ki.has(K_GLOW_SQUID) {
                glow_squid()
            } else {
                b
            }
        } else {
            match e.kind.as_str() {
                _ if e.flags() & 1 != 0 => 15,
                "vex" | "magma_cube" | "wither" | "blaze" | "allay" => 15,
                "glow_squid" => glow_squid(),
                _ => b,
            }
        };
        let packed = (u32::from(block) * 16) | ((u32::from(s) * 16) << 8);
        // The dragon is not a living-entity render: red overlay on damage only.
        let red = e.hurt_time > 0 || (e.death_time > 0 && e.kind != "ender_dragon");
        let mut white = 0.0f32;
        // Swallowed content of the sulfur cube (block or skull), resolved once for the frame.
        let swallowed = if e.kind == "sulfur_cube" { self.swallowed(e) } else { None };
        // Entity matrix (part 2 § 4.2); root imposed for an interface model.
        let mut m = self.gui_root.unwrap_or_else(|| Mat4::from_translation(Vec3::from(rel)));
        if living {
            // Lying in a bed: shifted towards the foot of the bed by the eye height − 0.1.
            if e.pose() == 2
                && let Some((sx, sz, _)) = env.bed
            {
                let off = e.standing_eye(&self.dims) - 0.1;
                m *= Mat4::from_translation(Vec3::new(-sx * off, 0.0, -sz * off));
            }
            // Scale of the living entity before the rotations (shulker attach pivot included);
            // brought back to 1 in the interface, like the game.
            if self.gui_root.is_none() {
                m *= Mat4::from_scale(Vec3::splat(living_scale(e)));
            }
            let ctx = RotCtx {
                shaking: self.shaking(e),
                upside_down,
                fall_flying,
                swim: state.swim,
                env,
                height: self.size_of(e).h,
            };
            pose_rotations(e, pt, body, &mut m, &ctx);
            shulker::attach(e, &mut m);
            m *= Mat4::from_scale(Vec3::new(-1.0, -1.0, 1.0));
            let mut scale = Vec3::ONE;
            if avatar {
                scale = Vec3::splat(0.9375);
            }
            if e.kind == "creeper" {
                let g = (e.swell.0 as f32 + pt * (e.swell.1 - e.swell.0) as f32) / 28.0;
                let w = 1.0 + (g * 100.0).sin() * g * 0.01;
                let h = g.clamp(0.0, 1.0).powi(4);
                let sxz = (1.0 + h * 0.4) * w;
                scale = Vec3::new(sxz, (1.0 + h * 0.1) / w, sxz);
                if (g * 10.0).floor() as i32 % 2 != 0 {
                    white = g.clamp(0.5, 1.0);
                }
            }
            if e.kind == "cave_spider" {
                scale *= 0.7;
            }
            animals::scale(e, pt, &mut scale);
            creatures::scale(e, pt, matches!(swallowed, Some(creatures::Swallowed::Block(_))), &mut scale);
            dragon::scale(e, pt, &mut scale);
            wither::scale(e, pt, &mut scale);
            m *= Mat4::from_scale(scale);
            m *= Mat4::from_translation(creatures::model_offset(e) + Vec3::new(0.0, -1.501, 0.0));
        } else if e.kind == "evoker_fangs" {
            m *= Mat4::from_rotation_y(deg(90.0 - e.yaw));
            m *= Mat4::from_scale(Vec3::new(-1.0, -1.0, 1.0));
            m *= Mat4::from_translation(Vec3::new(0.0, -1.501, 0.0));
        } else if matches!(e.kind.as_str(), "arrow" | "spectral_arrow") {
            // Yaw − 90° then pitch; shaking after the impact.
            let yaw = e.yaw_o + (e.yaw - e.yaw_o) * pt;
            let pitch = e.pitch_o + (e.pitch - e.pitch_o) * pt;
            m *= Mat4::from_rotation_y(deg(yaw - 90.0)) * Mat4::from_rotation_z(deg(pitch));
            let s = e.shake as f32 - pt;
            if s > 0.0 {
                m *= Mat4::from_rotation_z(deg(-(3.0 * s).sin() * s));
            }
        } else {
            m *= Mat4::from_rotation_y(deg(-e.yaw));
        }
        let overlay = (white * 15.0) as u32 | (if red { 3 } else { 10 }) << 16;
        let wool = e.meta_byte(18) as u8;
        // "jeb_" sheep: rainbow wool.
        let jeb = e.kind == "sheep" && custom_name(e).is_some_and(|n| n == "jeb_");
        let dissolve = dragon::dissolve(e, pt).and_then(|a| Some((a, self.atlas.rect(dragon::MASK)?)));
        let (mut parts_buf, mut vis_buf, mut world_m) = std::mem::take(&mut self.scratch);
        // Arms of the main layer (held items) and base name of the model (armors).
        let mut arms: [Option<HandRef>; 2] = [None, None];
        let mut head_m: Option<Mat4> = None;
        let mut base_model: Option<usize> = None;
        // Invisible living entity: body absent (culled translucent ghost if the local player
        // sees it, silhouette only if it glows); layers copied from the absent body absent; eyes,
        // swirl, armor and held items drawn.
        // The illusioner keeps its body (and its layers) even when invisible: illusion copies.
        let invisible = living && e.flags() & 0x20 != 0 && e.kind != "illusioner";
        let ghost = invisible && self.ghosts.contains(&e.uuid);
        let glows = self.glow.contains_key(&e.uuid);
        // Spectator player (seen by a spectator): head and hat only.
        let spectator_avatar = e.kind == "player" && self.spectators.contains(&e.uuid);
        let flag = self.opt.equip_flag;
        let equipment = |gl: &GpuLayer| if flag { gl.equipment } else { equipment_layer(&gl.name) };
        for li in 0..self.arena[layers].len() {
            let r = self.arena[layers][li];
            // Breeze: wind and eyes always drawn; copper golem eyes hidden. Wool of the
            // sheep and shell of the slime of an invisible but glowing entity: outline only.
            let hidden = invisible
                && li > 0
                && (!matches!(r.pass, Pass::Eyes | Pass::Swirl | Pass::Wind) || e.kind == "copper_golem")
                && !(r.pass == Pass::Emissive && e.kind == "breeze")
                && !equipment(&self.layers[r.layer]);
            let outline_only = hidden
                && glows
                && if flag {
                    self.layers[r.layer].outlined
                } else {
                    outlined_when_invisible(&self.layers[r.layer].name)
                };
            if hidden && !outline_only {
                continue;
            }
            let tint = match r.wool {
                Wool::None => 0xffff_ffff,
                Wool::Undercoat if wool & 15 == 0 && !jeb => continue,
                Wool::Coat if wool & 0x10 != 0 => continue,
                Wool::Undercoat | Wool::Coat if jeb => jeb_wool(state.age),
                Wool::Undercoat | Wool::Coat => wool_color(usize::from(wool & 15)),
                Wool::BodyDye(d) => match e.equipment[6].as_ref().and_then(|s| self.dyed(s)).unwrap_or(d) {
                    0 => continue,
                    c => c | 0xff00_0000,
                },
                Wool::Channel(ch) => creatures::tint(e, ch),
                Wool::Fixed(c) => c,
                Wool::Pulse(id) => match pulse_alpha(e, id, pt) {
                    a if a <= 1.0e-5 => continue,
                    a => 0x00ff_ffff | ((a.min(1.0) * 255.0) as u32) << 24,
                },
            };
            // Wet wolf shaking itself dry: body darkened (0.75 → 1 over the shake).
            let tint = if li == 0 && e.kind == "wolf" && e.extra[4] != 0.0 {
                let shade = (0.75 + (e.extra[3] + pt * (e.extra[2] - e.extra[3])) / 2.0 * 0.25).min(1.0);
                let g = (shade * 255.0) as u32;
                (tint & 0xff00_0000) | g << 16 | g << 8 | g
            } else {
                tint
            };
            let gl = &self.layers[r.layer];
            let mut pose = Pose::reset(&gl.baked, parts_buf, vis_buf);
            animate(&mut pose, e, &state, pt, living);
            if spectator_avatar {
                spectator_head_only(&mut pose);
            }
            if let Some(&[x, y]) = self.objects.guardian_eyes.get(&e.id) {
                pose.set("eye", |q| {
                    q[0] = x;
                    q[1] = y;
                });
            }
            let base = self.mats.len() as u32;
            // Part matrices, parent before child; hidden part → zero matrix.
            world_m.clear();
            world_m.resize(gl.baked.parts.len(), None);
            // Equipment layers (mount and wolf armor, cracks, saddles, decor,
            // harness): layer offset towards the camera like the armors, in the world.
            let layered = equipment(gl) && self.gui_root.is_none();
            let lay = if layered { self.layering } else { Mat4::IDENTITY };
            // Without offset, the product by the identity returns `pm` (up to the sign of zeros,
            // invisible: `identity_product_keeps_matrix`): skipped.
            let skip = flag && !layered;
            for (i, part) in gl.baked.parts.iter().enumerate() {
                let parent = if part.parent == u32::MAX { Some(m) } else { world_m[part.parent as usize] };
                let pm = parent.filter(|_| pose.visible[i]).map(|p| p * part_matrix(&pose.parts[i]));
                world_m[i] = pm;
                self.mats.push(match pm {
                    Some(pm) if skip => PartMat::new(&pm),
                    Some(pm) => PartMat::new(&(lay * pm)),
                    None => PartMat::zeroed(),
                });
            }
            let (pass, light_word, ov) = match r.pass {
                Pass::Eyes => (2, 0xf0f0, 10 << 16),
                // Texture offset in fractions of 1/65536 (u at the bottom, v at the top).
                Pass::Swirl => {
                    // Charged creeper: linear scroll; wither armor: oscillating u.
                    let (u, v) = if e.kind == "creeper" {
                        (state.age * 0.01 % 1.0, state.age * 0.01 % 1.0)
                    } else {
                        wither::swirl_offset(state.age)
                    };
                    let q = |f: f32| (f.rem_euclid(1.0) * 65535.0) as u32;
                    (7, 0xf0f0, q(u) | q(v) << 16)
                }
                // Damage overlay without white; no lightmap.
                Pass::Emissive => (1, packed | LIGHT_EMISSIVE, overlay & 0xffff_0000),
                // Breeze wind: scroll in u of 0.02 per tick.
                Pass::Wind => {
                    let u = (state.age * 0.02 % 1.0).rem_euclid(1.0);
                    (1, packed | LIGHT_FLAT | LIGHT_SCROLL, (u * 65535.0) as u32)
                }
                // Interior of the sulfur cube (layer 0 without content): fuse blinking.
                p if li == 0 && e.kind == "sulfur_cube" && e.equipment[6].is_none() => {
                    (p.index(), packed, creatures::fuse_overlay(e, pt, overlay))
                }
                // Animal equipment (saddle, armor, carpet, harness, cracks) and wolf collar:
                // drawn without damage overlay or white, like the game.
                p if (if flag {
                    gl.equipment || (e.kind == "wolf" && matches!(r.wool, Wool::Fixed(_)))
                } else {
                    no_damage_overlay(&e.kind, &gl.name, r.wool)
                }) =>
                {
                    (p.index(), packed, 10 << 16)
                }
                p => (p.index(), packed, overlay),
            };
            let tint = if r.pass == Pass::Swirl { 0xff80_8080 } else { tint };
            let mut color = (tint >> 16 & 0xff) | (tint & 0xff00) | (tint & 0xff) << 16 | (tint & 0xff00_0000);
            // Dissolve: white with decreasing alpha, mask offset in place of the overlay.
            let (pass, ov) = match dissolve {
                Some((alpha, mask)) if r.pass == Pass::Cutout => {
                    let d = |i: usize| u32::from((mask[i] - r.rect[i]) as i16 as u16);
                    color = 0x00ff_ffff | u32::from((alpha * 255.0) as u8) << 24;
                    (5, d(0) | d(1) << 16)
                }
                _ => (pass, ov),
            };
            let inst = Instance { base, rect: r.rect, color, light: light_word, overlay: ov };
            if outline_only {
                self.glow_only.push((0, gl.first, gl.count, inst));
            } else if !(invisible && li == 0) {
                // Rank of the layer in the entity (high 4 bits of the source): coplanar layers
                // (villager type, profession, level…) are drawn in the game's order, the last on
                // top, even after the cutouts are regrouped.
                self.instances[pass].push(((li.min(15) as u8) << 4, gl.first, gl.count, inst));
            } else if ghost {
                // White with alpha 0x26 multiplied into the tint.
                let a = (color >> 24) * 0x26 / 255;
                let inst = Instance { color: color & 0x00ff_ffff | a << 24, ..inst };
                self.instances[PASS_TRANSLUCENT_CULL].push((0, gl.first, gl.count, inst));
            } else if glows {
                self.glow_only.push((0, gl.first, gl.count, inst));
            }
            if li == 0 {
                base_model = Some(r.layer);
                // Body pose, copied onto the armor pieces.
                self.base_pose.0.clear();
                self.base_pose.0.extend_from_slice(&pose.parts);
                self.base_pose.1.clear();
                self.base_pose.1.extend_from_slice(&pose.visible);
                // Arms even when hidden (armor stand without arms): the game places the hand without
                // looking at visibility.
                let body =
                    pose.index("body").map(|i| world_m[i].unwrap_or_else(|| raw_world(&gl.baked, &pose.parts, m, i)));
                let right_arm_x = pose.index("right_arm").map_or(0.0, |i| pose.parts[i][3]);
                for (k, arm) in ["right_arm", "left_arm"].into_iter().enumerate() {
                    arms[k] = pose.index(arm).map(|i| {
                        let p = gl.baked.parts[i].parent;
                        let parent = if p == u32::MAX {
                            m
                        } else {
                            world_m[p as usize].unwrap_or_else(|| raw_world(&gl.baked, &pose.parts, m, p as usize))
                        };
                        HandRef { parent, arm: pose.parts[i], body, right_arm_x }
                    });
                }
                head_m = pose.index("head").and_then(|i| world_m[i]);
                self.base_world.clear();
                self.base_world.extend_from_slice(&world_m);
            }
            (parts_buf, vis_buf) = (pose.parts, pose.visible);
        }
        self.scratch = (parts_buf, vis_buf, world_m);
        if living {
            // Mushrooms of the mooshroom and pumpkin of the snow golem of an invisible
            // but glowing entity: placed then kept as outline only.
            let outline = invisible && glows && matches!(e.kind.as_str(), "mooshroom" | "snow_golem");
            let marks =
                outline.then(|| -> [usize; 7] { std::array::from_fn(|i| self.instances[GLOW_PASSES[i]].len()) });
            let arm = arms[0].as_ref().map(HandRef::tip);
            self.push_carried(e, m, arm, head_m, packed, overlay, invisible && !outline);
            if let Some(marks) = marks {
                for (k, &from) in marks.iter().enumerate() {
                    let list = &mut self.instances[GLOW_PASSES[k]];
                    let from = from.min(list.len());
                    self.glow_only.extend(list.drain(from..));
                }
            }
            if let Some(content) = swallowed {
                self.push_swallowed(e, content, m, pt, packed, overlay, items, walk_pos);
            }
            if let (Some(models), Some(base)) = (items, base_model) {
                self.push_mob_item(e, base, m, models, &state, pt, packed);
            }
        }
        self.dragon_effects(e, m, rel, pt, packed);
        if living && let Some(base) = base_model {
            // Spectator avatar: no layers (cape, armor, held items).
            let spectator = self.spectators.contains(&e.uuid);
            if !invisible && !spectator {
                self.push_cape(e, base, m, pt, packed);
            }
            if !spectator {
                self.push_armor(e, base, m, state.baby, packed);
                if let Some(h) = head_m {
                    self.push_head(e, m, h, items, packed, walk_pos);
                }
                if e.kind == "player" {
                    self.push_shoulder_parrots(e, m, &state, packed);
                }
                if avatar {
                    self.push_stuck(e, base, packed);
                    let skin = self.arena[layers].first().map_or([0.0; 4], |r| r.rect);
                    self.push_spin_and_ears(e, base, skin, m, &state, packed, overlay, invisible);
                }
                self.push_wings(e, m, pt, state.baby, packed);
            }
            if let Some(models) = items
                && !spectator
                && held_visible(e)
            {
                // Avatar looking through a spyglass: eye frame (head at pitch bounded
                // to −30°..90°, then head item frame).
                let eye = (avatar && state.using && state.swing < 1.0e-5)
                    .then(|| {
                        let hi = self.layers[base].baked.names.iter().position(|n| n == "head")?;
                        let mut h = self.base_pose.0[hi];
                        h[3] = h[3].clamp(-PI / 6.0, PI / 2.0);
                        let at = m
                            * part_matrix(&h)
                            * Mat4::from_translation(Vec3::new(0.0, -0.25, 0.0))
                            * Mat4::from_rotation_y(PI)
                            * Mat4::from_scale(Vec3::new(0.625, -0.625, -0.625));
                        Some((usize::from(e.meta_byte(8) & 2 != 0), at))
                    })
                    .flatten();
                let slim = self.layer_index.get("minecraft:player_slim#main") == Some(&base);
                self.push_held(e, models, arms, slim, eye, packed, &state);
            }
        }
    }

    /// Worn texture of an armor piece's trim (network slot), recoloured and
    /// placed in the atlas on first use.
    fn trim_rect(&mut self, stack: &rc_proto_v777::item::ItemStack, slot: usize) -> Option<[f32; 4]> {
        let b = stack.component(self.ids.of("minecraft:trim")?)?;
        let info = usize::try_from(stack.item).ok().and_then(|i| self.source.items.get(i))?;
        let ty = if slot == 3 { "humanoid_leggings" } else { "humanoid" };
        let atlas = &self.atlas;
        let (name, img) = crate::item_regs::worn_texture(b, &info.equip_asset, ty, |n| atlas.contains(n))?;
        if let Some(img) = img {
            self.atlas.insert(&name, img);
        }
        self.atlas.rect(&name)
    }

    /// Main arm on the right (player and mannequin: hand option; mobs: left-handed flag).
    fn right_handed(&self, e: &Entity) -> bool {
        match e.kind.as_str() {
            "player" | "mannequin" => e.meta_int(15).is_none_or(|v| v != 0),
            _ => e.meta_byte(15) & 2 == 0,
        }
    }

    /// Kinetic weapon (spear) held by the right or left arm.
    fn arm_kinetic(&self, e: &Entity, right: bool) -> Option<crate::hand::Kinetic> {
        let slot = usize::from(right != self.right_handed(e));
        let stack = e.equipment[slot].as_ref()?;
        let info = usize::try_from(stack.item).ok().and_then(|i| self.source.items.get(i));
        let view =
            crate::stack::StackRef { stack, info, ids: &self.ids, data: &self.source, hold: crate::stack::Hold::NONE };
        crate::hand::kinetic_of(&view, &self.ids)
    }

    /// Charge duration of a crossbow (ticks), Quick Charge included.
    fn crossbow_charge(&self, stack: &rc_proto_v777::item::ItemStack) -> i32 {
        let info = usize::try_from(stack.item).ok().and_then(|i| self.source.items.get(i));
        let view =
            crate::stack::StackRef { stack, info, ids: &self.ids, data: &self.source, hold: crate::stack::Hold::NONE };
        rc_assets::item::StackView::crossbow_charge(&view)
    }

    /// Worn armor (`entities.md` part 3 § 3.5): layers of the slot, animated like the
    /// body; order chest, legs, feet, head.
    /// Cape and elytra of an avatar (rectangles in the atlas). Player: textures downloaded
    /// from their UUID. Mannequin: those of the `textures` property of its profile, each
    /// replaced by the profile's patch if it gives one (game resource).
    fn avatar_extras(&mut self, e: &Entity) -> (Option<[f32; 4]>, Option<[f32; 4]>) {
        avatar_extras(&mut self.atlas, &self.custom_capes, &self.custom_elytras, e)
    }

    /// Cape of an avatar, player or mannequin (`entities.md` part 3 § 3.3): "cape" bit of the
    /// skin parts, no elytra; body pose copied, cape rotation composed with
    /// its pose.
    fn push_cape(&mut self, e: &Entity, base: usize, m: Mat4, pt: f32, light: u32) {
        if !shows_cape(e) {
            return;
        }
        let Some(rect) = self.avatar_extras(e).0 else { return };
        let Some(&layer) = self.layer_index.get("minecraft:player#cape") else { return };
        let chest = e.equipment[4]
            .as_ref()
            .and_then(|s| usize::try_from(s.item).ok())
            .and_then(|i| self.source.items.get(i))
            .and_then(|info| self.equipment.get(&info.equip_asset));
        if chest.is_some_and(|t| t.contains_key("wings")) {
            return;
        }
        let m = if chest.is_some_and(|t| t.contains_key("humanoid")) {
            m * Mat4::from_translation(Vec3::new(0.0, -0.053125, 0.06875))
        } else {
            m
        };
        // The game applies (flight ticks + pt)² / 100 even on the ground (≤ 0.01).
        let fly = e.pose_anim.fly_scale(pt);
        let (flap, lean, lean2) = e.cape_angles(pt, fly);
        let extra = Mat4::from_rotation_y(-std::f32::consts::PI)
            * Mat4::from_rotation_x((6.0 + lean / 2.0 + flap).to_radians())
            * Mat4::from_rotation_z((lean2 / 2.0).to_radians())
            * Mat4::from_rotation_y((180.0 - lean2 / 2.0).to_radians());
        let (parts_buf, vis_buf, mut world_m) = std::mem::take(&mut self.scratch);
        let gl = &self.layers[layer];
        let mut pose = Pose::reset(&gl.baked, parts_buf, vis_buf);
        let from = &self.layers[base].baked.known;
        for (k, &a) in gl.baked.known.iter().enumerate() {
            let b = from[k];
            if a != u16::MAX && b != u16::MAX {
                pose.parts[usize::from(a)] = self.base_pose.0[usize::from(b)];
                pose.visible[usize::from(a)] = self.base_pose.1[usize::from(b)];
            }
        }
        let first = self.mats.len() as u32;
        world_m.clear();
        world_m.resize(gl.baked.parts.len(), None);
        for (i, part) in gl.baked.parts.iter().enumerate() {
            let parent = if part.parent == u32::MAX { Some(m) } else { world_m[part.parent as usize] };
            // The cape is the only part with cubes in this layer.
            let own =
                if part.quads.is_empty() { part_matrix(&pose.parts[i]) } else { part_matrix(&pose.parts[i]) * extra };
            let pm = parent.filter(|_| pose.visible[i]).map(|p| p * own);
            world_m[i] = pm;
            self.mats.push(pm.map_or_else(PartMat::zeroed, |pm| PartMat::new(&pm)));
        }
        self.scratch = (pose.parts, pose.visible, world_m);
        let (gl_first, gl_count) = (gl.first, gl.count);
        // Cape: opaque without cutout, back faces culled.
        self.instances[PASS_SOLID].push((
            0,
            gl_first,
            gl_count,
            Instance { base: first, rect, color: 0xffff_ffff, light, overlay: 10 << 16 },
        ));
    }

    /// Flames of an entity on fire (flag 0x01, except fire-immune kinds and falling blocks):
    /// quads turned towards the camera around Y, stacked every 0.45 and shrinking,
    /// alternating frames of the two fire textures, block light 15; cutout with
    /// back faces culled.
    fn push_flame(
        &mut self,
        e: &Entity,
        ki: Option<KindInfo>,
        rel: [f32; 3],
        world: [f64; 3],
        light: &dyn Fn(i32, i32, i32) -> (u8, u8),
    ) {
        // Blaze: "on fire" = charged (bit 1 of meta 16), unless immune; never for a
        // spectator.
        let burning = match ki {
            Some(k) if k.has(K_BLAZE) => e.meta_byte(16) & 1 != 0,
            Some(k) => e.flags() & 1 != 0 && !k.has(K_FLAME_IMMUNE),
            None if e.kind == "blaze" => e.meta_byte(16) & 1 != 0,
            None => e.flags() & 1 != 0 && !FLAME_IMMUNE.contains(&e.kind.as_str()),
        };
        if !burning || self.spectators.contains(&e.uuid) {
            return;
        }
        let (Some(f0), Some(f1)) = (self.atlas.rect("#fire_0"), self.atlas.rect("#fire_1")) else { return };
        let size = self.size_of(e);
        let (w, mut h) = (size.w, size.h);
        let s = w * 1.4;
        if s <= 0.0 {
            return;
        }
        h /= s;
        let (_, sky) = light(world[0].floor() as i32, world[1].floor() as i32, world[2].floor() as i32);
        let lw = 0xf0 | (u32::from(sky) * 16) << 8;
        let base = Mat4::from_translation(Vec3::from(rel))
            * Mat4::from_scale(Vec3::splat(s))
            * Mat4::from_rotation_y(self.cam_yaw)
            * Mat4::from_translation(Vec3::new(0.0, 0.0, 0.3 - (h as i32) as f32 * 0.02));
        let (mut r, mut yo, mut zo, mut ss) = (0.5f32, 0.0f32, 0.0f32, 0u32);
        while h > 0.0 {
            let mut rect = if ss % 2 == 0 { f0 } else { f1 };
            // Texture flipped horizontally on every other pair of quads.
            if (ss / 2) % 2 != 0 {
                rect = [rect[0] + rect[2], rect[1], -rect[2], rect[3]];
            }
            // Orb quad (−0.5, −0.25)…(0.5, 0.75) → (−r, −yo)…(r, 1.4 − yo) at z = zo.
            let m = base
                * Mat4::from_translation(Vec3::new(0.0, -yo, zo))
                * Mat4::from_scale(Vec3::new(2.0 * r, 1.4, 1.0))
                * Mat4::from_translation(Vec3::new(0.0, 0.25, 0.0));
            let b = self.mats.len() as u32;
            self.mats.push(PartMat::new(&m));
            self.instances[PASS_CUTOUT_CULL].push((
                1,
                0,
                6,
                Instance { base: b, rect, color: 0xffff_ffff, light: lw, overlay: 10 << 16 },
            ));
            h -= 0.45;
            yo -= 0.45;
            r *= 0.9;
            zo -= 0.03;
            ss += 1;
        }
    }

    /// Layers of an armor piece (base model, network slot, item, baby), resolved
    /// once: index in `armor_arena`.
    fn armor_layers(&mut self, base: usize, slot: usize, item: i32, baby: bool) -> usize {
        let key = (base as u32, slot as u8, item, baby);
        if let Some(&i) = self.armor_index.get(&key) {
            return i;
        }
        // (network slot, equipment slot + 1, layer, layer type).
        const SLOTS: [(usize, u8, &str, &str); 4] = [
            (4, 4, "chestplate", "humanoid"),
            (3, 3, "leggings", "humanoid_leggings"),
            (2, 2, "boots", "humanoid"),
            (5, 5, "helmet", "humanoid"),
        ];
        let mut out = Vec::new();
        let found = SLOTS.iter().find(|s| s.0 == slot).and_then(|&(_, equip, suffix, ty)| {
            let info = usize::try_from(item).ok().and_then(|i| self.source.items.get(i))?;
            if info.equip_slot != equip || info.equip_asset.is_empty() {
                return None;
            }
            let prefix = self.layers[base].name.strip_suffix("#main")?;
            let ty = if baby && ty == "humanoid" { "humanoid_baby" } else { ty };
            let layer = *self.layer_index.get(&format!("{prefix}#{suffix}"))?;
            let defs = self.equipment.get(&info.equip_asset)?.get(ty)?.clone();
            Some((layer, ty, defs))
        });
        if let Some((layer, ty, defs)) = found {
            for (tex, dye) in defs {
                if let Some(rect) = self.atlas.rect(&format!("entity/equipment/{ty}/{tex}")) {
                    out.push(ArmorLayer { layer, rect, dye });
                }
            }
        }
        self.armor_arena.push(out);
        let i = self.armor_arena.len() - 1;
        self.armor_index.insert(key, i);
        i
    }

    /// Tint (colour component) of a stack, if there is one.
    fn dyed(&self, stack: &rc_proto_v777::item::ItemStack) -> Option<u32> {
        let info = usize::try_from(stack.item).ok().and_then(|i| self.source.items.get(i));
        let view =
            crate::stack::StackRef { stack, info, ids: &self.ids, data: &self.source, hold: crate::stack::Hold::NONE };
        rc_assets::item::StackView::dyed_color(&view)
    }

    /// Worn armor (`entities.md` part 3 § 3.5): body pose copied part by part
    /// (like the game, without re-animating), one instance per texture layer (same matrices).
    fn push_armor(&mut self, e: &Entity, base: usize, m: Mat4, baby: bool, light: u32) {
        for slot in [4, 3, 2, 5] {
            let Some(stack) = &e.equipment[slot] else { continue };
            let idx = self.armor_layers(base, slot, stack.item, baby);
            let n = self.armor_arena[idx].len();
            if n == 0 {
                continue;
            }
            let dyed = if self.armor_arena[idx].iter().any(|a| a.dye.is_some()) { self.dyed(stack) } else { None };
            let layer = self.armor_arena[idx][0].layer;
            let (parts_buf, vis_buf, mut world_m) = std::mem::take(&mut self.scratch);
            let gl = &self.layers[layer];
            let mut pose = Pose::reset(&gl.baked, parts_buf, vis_buf);
            let from = &self.layers[base].baked.known;
            for (k, &a) in gl.baked.known.iter().enumerate() {
                let b = from[k];
                if a != u16::MAX && b != u16::MAX {
                    pose.parts[usize::from(a)] = self.base_pose.0[usize::from(b)];
                    pose.visible[usize::from(a)] = self.base_pose.1[usize::from(b)];
                }
            }
            let first = self.mats.len() as u32;
            world_m.clear();
            world_m.resize(gl.baked.parts.len(), None);
            // Layer offset of the armors (towards the camera), in the world only.
            let lay = if self.gui_root.is_some() { Mat4::IDENTITY } else { self.layering };
            for (i, part) in gl.baked.parts.iter().enumerate() {
                let parent = if part.parent == u32::MAX { Some(m) } else { world_m[part.parent as usize] };
                let pm = parent.filter(|_| pose.visible[i]).map(|p| p * part_matrix(&pose.parts[i]));
                world_m[i] = pm;
                self.mats.push(pm.map_or_else(PartMat::zeroed, |pm| PartMat::new(&(lay * pm))));
            }
            self.scratch = (pose.parts, pose.visible, world_m);
            let (gl_first, gl_count) = (gl.first, gl.count);
            // Glint in the shader, on the first drawn layer only (the following ones
            // cover it where they are opaque).
            // Trim (not on baby armor): the glint passes from the layers to the trim.
            let trim = if baby { None } else { self.trim_rect(stack, slot) };
            let foil = if self.foil(stack) { GLINT_ARMOR } else { 0 };
            let mut glint = if trim.is_some() { 0 } else { foil };
            for j in 0..n {
                let a = self.armor_arena[idx][j];
                let tint = match a.dye {
                    None => 0xffff_ffff,
                    Some(d) => match dyed.unwrap_or(d) {
                        0 => continue,
                        c => c | 0xff00_0000,
                    },
                };
                let color = (tint >> 16 & 0xff) | (tint & 0xff00) | (tint & 0xff) << 16 | (tint & 0xff00_0000);
                self.instances[0].push((
                    0,
                    gl_first,
                    gl_count,
                    Instance { base: first, rect: a.rect, color, light: light | glint, overlay: 10 << 16 },
                ));
                glint = 0;
            }
            if let Some(rect) = trim {
                // Trim: drawn by the cutout, then the glint in a separate pass (equal depth)
                // over the whole piece, regardless of the trim's transparency.
                let inst = Instance { base: first, rect, color: 0xffff_ffff, light: light | foil, overlay: 10 << 16 };
                self.instances[0].push((0, gl_first, gl_count, Instance { light, ..inst }));
                if foil != 0 {
                    self.instances[6].push((0, gl_first, gl_count, inst));
                }
            }
        }
    }

    /// Item worn on the head (head slot, excluding armor drawn by the body): skulls and
    /// heads by their model (kind-specific lift, scale 1.1875; jaw and ears
    /// following the walk; player head with the skin of its profile), other items by their model
    /// in the "head" context (0.625, flipped). Villager: head lowered; piglins: widened.
    fn push_head(
        &mut self,
        e: &Entity,
        m: Mat4,
        head: Mat4,
        models: Option<&rc_assets::item::ItemModels>,
        light: u32,
        walk: f32,
    ) {
        if !matches!(
            e.kind.as_str(),
            "player"
                | "mannequin"
                | "armor_stand"
                | "zombie"
                | "husk"
                | "drowned"
                | "zombie_villager"
                | "skeleton"
                | "stray"
                | "bogged"
                | "parched"
                | "wither_skeleton"
                | "piglin"
                | "piglin_brute"
                | "zombified_piglin"
                | "pillager"
                | "vindicator"
                | "evoker"
                | "illusioner"
                | "villager"
                | "wandering_trader"
                | "copper_golem"
        ) {
            return;
        }
        let Some(stack) = &e.equipment[5] else { return };
        let source = self.source.clone();
        let Some(name) =
            usize::try_from(stack.item).ok().and_then(|i| items_in(&source, self.item_idx())?.get(i))
        else {
            return;
        };
        let info = usize::try_from(stack.item).ok().and_then(|i| source.items.get(i));
        let (y_off, skull_y, hs, vs) = match e.kind.as_str() {
            "villager" | "zombie_villager" => (-0.117_187_5, -0.074_218_75, 1.0, 1.0),
            "piglin" | "piglin_brute" | "zombified_piglin" => (0.0, 0.0, 1.001_953_1, 1.0),
            _ => (0.0, 0.0, 1.0, 1.0),
        };
        let head = m * Mat4::from_scale(Vec3::new(hs, vs, hs)) * m.inverse() * head * head_item_frame(&e.kind);
        let view =
            crate::stack::StackRef { stack, info, ids: &self.ids, data: &source, hold: crate::stack::Hold::NONE };
        if let Some(kind) = SKULLS.iter().position(|s| s.0 == name) {
            let mtx =
                head * Mat4::from_translation(Vec3::new(0.0, skull_y, 0.0)) * Mat4::from_scale(Vec3::splat(1.1875));
            let key = Self::worn_head_key(kind, models, &view);
            self.push_worn_skull(kind, key, mtx, light, walk, false);
            return;
        }
        // Head armor (drawn by the armor layer): nothing here.
        let armor = info.is_some_and(|i| {
            i.equip_slot == 5 && self.equipment.get(&i.equip_asset).is_some_and(|t| t.contains_key("humanoid"))
        });
        let Some(models) = models else { return };
        let Some(model) = view.model() else { return };
        if armor {
            return;
        }
        let mtx = head
            * Mat4::from_translation(Vec3::new(0.0, -0.25 + y_off, 0.0))
            * Mat4::from_rotation_y(PI)
            * Mat4::from_scale(Vec3::new(0.625, -0.625, -0.625));
        let (mut layers, mut key) = std::mem::take(&mut self.held_scratch);
        layers.clear();
        key.clear();
        let env = self.owner_env(e, "head");
        models.layers(name, &model, &view, &env, env.seed(), &mut layers, &mut key);
        for l in &layers {
            let b = models.baked(l.model).unwrap_or(&models.missing);
            let d = Mat4::from_cols_array_2d(&b.displays[6]).transpose();
            self.push_item_layer(models, l, mtx * d * Mat4::from_cols_array_2d(&l.local).transpose(), light);
        }
        self.held_scratch = (layers, key);
    }

    /// Parrots on a player's shoulders (variants, values 19 left and 20 right): at
    /// ±0.4 and 1.5 below the pivot (1.3 crouching), head following the player's, tail at the walk
    /// rhythm, wings folded.
    fn push_shoulder_parrots(&mut self, e: &Entity, m: Mat4, s: &AnimState, light: u32) {
        const PARROTS: [&str; 5] = ["red_blue", "blue", "green", "yellow_blue", "grey"];
        let Some(&layer) = self.layer_index.get("minecraft:parrot#main") else { return };
        for (index, left) in [(19u8, true), (20, false)] {
            let Some(rc_proto_v777::entity::Meta::OptionalInt(Some(v))) = e.meta.get(&index) else { continue };
            let Some(rect) = self.atlas.rect(&format!("entity/parrot/parrot_{}", PARROTS[(*v).clamp(0, 4) as usize]))
            else {
                continue;
            };
            let at = m * Mat4::from_translation(Vec3::new(
                if left { 0.4 } else { -0.4 },
                if s.crouching { -1.3 } else { -1.5 },
                0.0,
            ));
            let (pitch, yaw) = (s.pitch, s.yaw);
            let tail = (s.walk_pos * 0.6662).cos() * 0.3 * s.walk_speed;
            self.push_posed(layer, rect, at, 0, light, |p| {
                p.set("head", |h| {
                    h[3] = pitch;
                    h[4] = yaw;
                });
                p.set("tail", |t| t[3] += tail);
                p.set("left_wing", |w| w[5] = -0.0873);
                p.set("right_wing", |w| w[5] = 0.0873);
            });
        }
    }

    /// Avatar: whirl of the spin attack (two boxes spinning at −50 and −55° per
    /// tick); ears of the "deadmau5" player (head pose copied, opaque skin, absent when
    /// invisible).
    #[allow(clippy::too_many_arguments)]
    fn push_spin_and_ears(
        &mut self,
        e: &Entity,
        base: usize,
        skin: [f32; 4],
        m: Mat4,
        s: &AnimState,
        light: u32,
        overlay: u32,
        invisible: bool,
    ) {
        if e.meta_byte(8) & 4 != 0
            && let (Some(&layer), Some(rect)) =
                (self.layer_index.get("minecraft:spin_attack#main"), self.atlas.rect("entity/trident/trident_riptide"))
        {
            let age = s.age;
            self.push_posed(layer, rect, m, 0, light, |p| {
                for (i, name) in ["box0", "box1"].into_iter().enumerate() {
                    let a = age * -(45.0 + (i as f32 + 1.0) * 5.0);
                    let wrapped = (a + 180.0).rem_euclid(360.0) - 180.0;
                    p.set(name, |b| b[4] = deg(wrapped));
                }
            });
        }
        if invisible || e.kind != "player" || !self.extra_ears.contains(&e.uuid) {
            return;
        }
        let rect = skin;
        let Some(&layer) = self.layer_index.get("minecraft:player#ears") else { return };
        let (parts_buf, vis_buf, mut world_m) = std::mem::take(&mut self.scratch);
        let gl = &self.layers[layer];
        let mut pose = Pose::reset(&gl.baked, parts_buf, vis_buf);
        let from = &self.layers[base].baked.known;
        for (k, &a) in gl.baked.known.iter().enumerate() {
            let b = from[k];
            if a != u16::MAX && b != u16::MAX {
                pose.parts[usize::from(a)] = self.base_pose.0[usize::from(b)];
                pose.visible[usize::from(a)] = self.base_pose.1[usize::from(b)];
            }
        }
        let first = self.mats.len() as u32;
        world_m.clear();
        world_m.resize(gl.baked.parts.len(), None);
        for (i, part) in gl.baked.parts.iter().enumerate() {
            let parent = if part.parent == u32::MAX { Some(m) } else { world_m[part.parent as usize] };
            let pm = parent.filter(|_| pose.visible[i]).map(|p| p * part_matrix(&pose.parts[i]));
            world_m[i] = pm;
            self.mats.push(pm.map_or_else(PartMat::zeroed, |pm| PartMat::new(&pm)));
        }
        let (gl_first, gl_count) = (gl.first, gl.count);
        self.scratch = (pose.parts, pose.visible, world_m);
        self.instances[PASS_SOLID].push((
            0,
            gl_first,
            gl_count,
            Instance { base: first, rect, color: 0xffff_ffff, light, overlay: overlay & 0xffff_0000 },
        ));
    }

    /// Matrix (world) of a part of the main layer of the current entity.
    fn base_part(&self, base: usize, name: &str) -> Option<Mat4> {
        self.layers[base]
            .baked
            .names
            .iter()
            .position(|n| n == name)
            .and_then(|i| self.base_world.get(i).copied().flatten())
    }

    /// Stack of the main hand drawn in the "ground" context under matrix `m`.
    fn push_stack_ground(
        &mut self,
        e: &Entity,
        models: &rc_assets::item::ItemModels,
        stack: &rc_proto_v777::item::ItemStack,
        m: Mat4,
        light: u32,
    ) {
        let source = self.source.clone();
        let info = usize::try_from(stack.item).ok().and_then(|i| source.items.get(i));
        let view =
            crate::stack::StackRef { stack, info, ids: &self.ids, data: &source, hold: crate::stack::Hold::NONE };
        let Some(model) = view.model() else { return };
        let name = usize::try_from(stack.item)
            .ok()
            .and_then(|i| items_in(&source, self.item_idx())?.get(i))
            .map_or("", String::as_str);
        let (mut layers, mut key) = std::mem::take(&mut self.held_scratch);
        layers.clear();
        key.clear();
        let env = self.owner_env(e, "ground");
        models.layers(name, &model, &view, &env, env.seed(), &mut layers, &mut key);
        for l in &layers {
            let b = models.baked(l.model).unwrap_or(&models.missing);
            let d = Mat4::from_cols_array_2d(&b.displays[1]).transpose();
            self.push_item_layer(models, l, m * d * Mat4::from_cols_array_2d(&l.local).transpose(), light);
        }
        self.held_scratch = (layers, key);
    }

    /// Item of the main hand held by a mob outside the arms: villager and trader (arms
    /// crossed), witch (arms crossed; potion at the tip of the nose), fox (mouth, follows the head),
    /// sitting panda (paws; chewed during the meal), dolphin (snout depending on pitch).
    #[allow(clippy::too_many_arguments)]
    fn push_mob_item(
        &mut self,
        e: &Entity,
        base: usize,
        m: Mat4,
        models: &rc_assets::item::ItemModels,
        s: &AnimState,
        pt: f32,
        light: u32,
    ) {
        if !matches!(e.kind.as_str(), "villager" | "wandering_trader" | "witch" | "fox" | "panda" | "dolphin") {
            return;
        }
        let Some(stack) = e.equipment[0].clone() else { return };
        let mtx = match e.kind.as_str() {
            "witch" if self.holds(e, 0, "potion") => {
                let Some(nose) = self.base_part(base, "nose") else { return };
                nose * Mat4::from_translation(Vec3::new(0.0625, 0.25, 0.0))
                    * Mat4::from_rotation_z(deg(180.0))
                    * Mat4::from_rotation_x(deg(140.0))
                    * Mat4::from_rotation_z(deg(10.0))
                    * Mat4::from_rotation_x(deg(180.0))
            }
            "villager" | "wandering_trader" | "witch" => {
                let Some(arms) = self.base_part(base, "arms") else { return };
                arms * Mat4::from_rotation_x(0.75)
                    * Mat4::from_scale(Vec3::splat(1.07))
                    * Mat4::from_translation(Vec3::new(0.0, 0.13, -0.34))
                    * Mat4::from_rotation_x(PI)
            }
            "fox" => {
                let Some(hi) = self.layers[base].baked.names.iter().position(|n| n == "head") else { return };
                let h = self.base_pose.0[hi];
                let sleeping = e.meta_byte(19) & 32 != 0;
                let roll = (e.extra[1] + pt * (e.extra[0] - e.extra[1])) * 0.11 * PI;
                let mut t = m * Mat4::from_translation(Vec3::new(h[0], h[1], h[2]) / 16.0);
                if s.baby {
                    t *= Mat4::from_scale(Vec3::splat(0.75));
                }
                t *= Mat4::from_rotation_z(roll) * Mat4::from_rotation_y(s.yaw) * Mat4::from_rotation_x(s.pitch);
                let off = match (s.baby, sleeping) {
                    (true, true) => Vec3::new(0.4, 0.26, 0.15),
                    (true, false) => Vec3::new(0.06, 0.26, -0.5),
                    (false, true) => Vec3::new(0.46, 0.26, 0.22),
                    (false, false) => Vec3::new(0.06, 0.27, -0.5),
                };
                t *= Mat4::from_translation(off) * Mat4::from_rotation_x(deg(90.0));
                if sleeping {
                    t *= Mat4::from_rotation_z(deg(90.0));
                }
                t
            }
            "panda" => {
                // Sitting (flag 8, value 23) and not scared; meal in progress (value 20 > 0).
                if e.meta_byte(23) & 8 == 0 || animals::panda_scared(e) {
                    return;
                }
                let (mut y, mut z) = (1.4f32, -0.6f32);
                if e.meta_int(20).unwrap_or(0) > 0 {
                    let w = (s.age * 0.6).sin();
                    z -= 0.2 * w + 0.2;
                    y -= 0.09 * w;
                }
                m * Mat4::from_translation(Vec3::new(0.1, y, z))
            }
            _ => {
                // Dolphin: pitch of the entity in degrees.
                let x_rot = e.pitch_o + pt * (e.pitch - e.pitch_o);
                let k = x_rot.abs() / 60.0;
                let o = if x_rot < 0.0 {
                    Vec3::new(0.0, 1.0 - k * 0.5, -1.0 + k * 0.5)
                } else {
                    Vec3::new(0.0, 1.0 + k * 0.8, -1.0 + k * 0.2)
                };
                m * Mat4::from_translation(o)
            }
        };
        self.push_stack_ground(e, models, &stack, mtx, light);
    }

    /// Arrows (value 12) and stingers (13) stuck in an avatar: Java generator seeded by
    /// the network id (one per kind), random body part (head, body, left and right
    /// arm, left and right leg), point drawn in its cube (stingers: brought onto a face,
    /// 1 or 0.5 like the game), oriented towards the centre of the cube.
    fn push_stuck(&mut self, e: &Entity, base: usize, light: u32) {
        const PARTS: [&str; 6] = ["head", "body", "left_arm", "right_arm", "left_leg", "right_leg"];
        let source = self.source.clone();
        let Some(def) = source.entity_layer(&self.layers[base].name) else { return };
        for (index, model, tex, surface) in [
            (12u8, "minecraft:arrow#main", "entity/projectiles/arrow", false),
            (13, "minecraft:bee_stinger#main", "entity/bee/bee_stinger", true),
        ] {
            let count = e.meta_int(index).unwrap_or(0);
            if count <= 0 {
                continue;
            }
            let (Some(&layer), Some(rect)) = (self.layer_index.get(model), self.atlas.rect(tex)) else { continue };
            let mut rng = rc_audio::JavaRandom::new(i64::from(e.id));
            for _ in 0..count.min(256) {
                let name = PARTS[rng.next_int_bounded(6) as usize];
                let Some(pi) = def.parts.iter().position(|p| p.name == name) else { return };
                let cubes = &def.parts[pi].cubes;
                if cubes.is_empty() {
                    return;
                }
                let c = cubes[rng.next_int_bounded(cubes.len() as i32) as usize];
                let mut mid = [rng.next_float(), rng.next_float(), rng.next_float()];
                if surface {
                    let plane = rng.next_int_bounded(3) as usize;
                    mid[plane] = if mid[plane] > 0.5 { 1.0 } else { 0.5 };
                }
                let Some(part_m) = self.layers[base]
                    .baked
                    .names
                    .iter()
                    .position(|n| n == name)
                    .and_then(|i| self.base_world.get(i).copied().flatten())
                else {
                    continue;
                };
                let at = Vec3::from(std::array::from_fn(|k| (c.from[k] + mid[k] * c.size[k]) / 16.0));
                let d = mid.map(|v| -(v * 2.0 - 1.0));
                let y_rot = d[0].atan2(d[2]).to_degrees();
                let x_rot = d[1].atan2((d[0] * d[0] + d[2] * d[2]).sqrt()).to_degrees();
                let mtx = part_m
                    * Mat4::from_translation(at)
                    * Mat4::from_rotation_y(deg(y_rot - 90.0))
                    * Mat4::from_rotation_z(deg(x_rot));
                self.push_posed(layer, rect, mtx, PASS_CUTOUT_CULL, light, |_| {});
            }
        }
    }

    /// Worn skull (index into `SKULLS`) under matrix `mtx` (scale 1.1875 included). Player
    /// head with profile: skin of the profile, translucent (cutout if `profile_cutout`, sulfur cube);
    /// otherwise cutout skull with layer offset towards the camera (in the world), like the game.
    /// Dragon jaw and piglin ears at the rhythm of `walk`.
    /// `key`: profile of the player head (`worn_head_key`).
    fn push_worn_skull(
        &mut self,
        kind: usize,
        key: Option<u128>,
        mtx: Mat4,
        light: u32,
        walk: f32,
        profile_cutout: bool,
    ) {
        let Some(&layer) = self.layer_index.get(SKULLS[kind].1) else { return };
        let (rect, pass, offset) = match key {
            Some(k) => {
                (self.head_skin(k).or_else(|| self.atlas.rect(SKULLS[6].2)), usize::from(!profile_cutout), false)
            }
            None => (self.atlas.rect(SKULLS[kind].2), 0, true),
        };
        let Some(rect) = rect else { return };
        let lay = if !offset || self.gui_root.is_some() { Mat4::IDENTITY } else { self.layering };
        self.push_posed(layer, rect, lay * mtx, pass, light, |p| match kind {
            4 => p.set("jaw", |q| q[3] = ((walk * PI * 0.2).sin() + 1.0) * 0.2),
            5 => {
                p.set("left_ear", |q| q[5] = -((walk * PI * 0.2 * 1.2).cos() + 2.5) * 0.2);
                p.set("right_ear", |q| q[5] = ((walk * PI * 0.2).cos() + 2.5) * 0.2);
            }
            _ => {}
        });
    }

    /// Instance of a baked layer (white, no overlay) placed by `anim` under matrix `m`.
    fn push_posed(
        &mut self,
        layer: usize,
        rect: [f32; 4],
        m: Mat4,
        pass: usize,
        light: u32,
        anim: impl FnOnce(&mut Pose<'_>),
    ) {
        let (parts_buf, vis_buf, mut world_m) = std::mem::take(&mut self.scratch);
        let gl = &self.layers[layer];
        let mut pose = Pose::reset(&gl.baked, parts_buf, vis_buf);
        anim(&mut pose);
        let base = self.mats.len() as u32;
        world_m.clear();
        world_m.resize(gl.baked.parts.len(), None);
        for (i, part) in gl.baked.parts.iter().enumerate() {
            let parent = if part.parent == u32::MAX { Some(m) } else { world_m[part.parent as usize] };
            let pm = parent.filter(|_| pose.visible[i]).map(|p| p * part_matrix(&pose.parts[i]));
            world_m[i] = pm;
            self.mats.push(pm.map_or_else(PartMat::zeroed, |pm| PartMat::new(&pm)));
        }
        let (first, count) = (gl.first, gl.count);
        self.scratch = (pose.parts, pose.visible, world_m);
        self.instances[pass].push((
            0,
            first,
            count,
            Instance { base, rect, color: 0xffff_ffff, light, overlay: 10 << 16 },
        ));
    }

    /// Profile of a worn skull (index into `SKULLS`): player head only.
    fn worn_head_key(
        kind: usize,
        models: Option<&rc_assets::item::ItemModels>,
        view: &crate::stack::StackRef<'_>,
    ) -> Option<u128> {
        if kind != 6 {
            return None;
        }
        Self::held_head_key(models?, SKULLS[6].0, view)
    }

    /// Identity of the player head of a stack (data of the "player head" special model).
    fn held_head_key(
        models: &rc_assets::item::ItemModels,
        name: &str,
        view: &crate::stack::StackRef<'_>,
    ) -> Option<u128> {
        let model = view.model()?;
        let (mut layers, mut key) = (Vec::new(), Vec::new());
        models.layers(name, &model, view, &HeldEnv("head"), 0, &mut layers, &mut key);
        layers.iter().find(|l| l.special == 2).and_then(|l| crate::item_special::head_key_of(&l.tints))
    }

    /// Layers of a mannequin: the game renders its profile without an online lookup. `textures`
    /// property (downloaded skin, profile model) then patch (body texture, slim/wide
    /// model) over the default skin drawn from the profile (`base` = its layers).
    fn mannequin_layers(&mut self, e: &Entity, base: usize) -> usize {
        let Some(p) = mannequin_profile(e) else { return base };
        let slim_layer = self.layer_index.get("minecraft:player_slim#main").copied();
        let is_slim = |r: &Resolved| Some(r.layer) == slim_layer;
        let Some(&def) = self.arena[base].first() else { return base };
        let (mut rect, mut slim) = (def.rect, is_slim(&def));
        if let Some(t) = &p.textures {
            let key = textures_key(t);
            if let Some(&i) = self.custom_skins.get(&key) {
                if let Some(r) = self.arena.get(i).and_then(|a| a.first()) {
                    (rect, slim) = (r.rect, is_slim(r));
                }
            } else if self.asked_heads.insert(key) {
                self.pending_heads.push((key, t.clone()));
            }
        }
        if let Some(id) = &p.body {
            let name = id.strip_prefix("minecraft:").unwrap_or(id);
            if let Some(r) = self.atlas.rect(name) {
                rect = r;
            }
        }
        slim = p.slim.unwrap_or(slim);
        if rect == def.rect && slim == is_slim(&def) {
            return base;
        }
        let key = (rect.map(f32::to_bits), slim);
        if let Some(&i) = self.avatar_layers.get(&key) {
            return i;
        }
        let model = if slim { "minecraft:player_slim#main" } else { "minecraft:player#main" };
        let Some(&layer) = self.layer_index.get(model) else { return base };
        self.arena.push(vec![Resolved { layer, rect, pass: Pass::Translucent, wool: Wool::None }]);
        self.avatar_layers.insert(key, self.arena.len() - 1);
        self.arena.len() - 1
    }

    /// Downloaded skin of a head (atlas rectangle), requested on first use.
    fn head_skin(&mut self, key: u128) -> Option<[f32; 4]> {
        if let Some(&i) = self.custom_skins.get(&key) {
            return self.arena.get(i).and_then(|a| a.first()).map(|r| r.rect);
        }
        if self.asked_heads.insert(key)
            && let Some(t) = crate::item_special::head_textures(key)
        {
            self.pending_heads.push((key, t));
        }
        None
    }

    /// Carried blocks: enderman block (in front of it, tilted), poppy offered by the iron
    /// golem (at the tip of the right arm), carved pumpkin of the snow golem (on the head, hidden
    /// when invisible; red damage tint).
    #[allow(clippy::too_many_arguments)]
    fn push_carried(
        &mut self,
        e: &Entity,
        m: Mat4,
        right_arm: Option<Mat4>,
        head: Option<Mat4>,
        light: u32,
        overlay: u32,
        invisible: bool,
    ) {
        match e.kind.as_str() {
            "enderman" => {
                let Some(rc_proto_v777::entity::Meta::BlockState(Some(s))) = e.meta.get(&16) else { return };
                let b = m
                    * Mat4::from_translation(Vec3::new(0.0, 0.6875, -0.75))
                    * Mat4::from_rotation_x(deg(20.0))
                    * Mat4::from_rotation_y(deg(45.0))
                    * Mat4::from_translation(Vec3::new(0.25, 0.1875, 0.25))
                    * Mat4::from_scale(Vec3::new(-0.5, -0.5, 0.5))
                    * Mat4::from_rotation_y(deg(90.0));
                self.push_block(*s as u32, b, light, 10 << 16);
            }
            "iron_golem" if e.extra[0] > 0.0 => {
                let (Some(arm), Some(s)) = (right_arm, self.held_blocks[0]) else { return };
                let b = arm
                    * Mat4::from_translation(Vec3::new(-1.1875 + 0.5, 1.0625 + 0.5, -0.9375 + 0.5))
                    * Mat4::from_scale(Vec3::splat(0.5))
                    * Mat4::from_rotation_x(deg(-90.0))
                    * Mat4::from_translation(Vec3::splat(-0.5));
                self.push_block(s, b, light, 10 << 16);
            }
            "snow_golem" if e.meta_byte(16) & 0x10 != 0 && !invisible => {
                let (Some(h), Some(s)) = (head, self.held_blocks[1]) else { return };
                let b = h
                    * Mat4::from_translation(Vec3::new(0.0, -0.34375, 0.0))
                    * Mat4::from_rotation_y(PI)
                    * Mat4::from_scale(Vec3::new(0.625, -0.625, -0.625))
                    * Mat4::from_translation(Vec3::splat(-0.5));
                self.push_block(s, b, light, overlay & 0xffff_0000);
            }
            // Copper golem: block placed on the antenna (saddle slot), default state of the
            // block with the same name as the item, flipped around its centre.
            "copper_golem" => {
                let Some(stack) = &e.equipment[7] else { return };
                let item = stack.item;
                let idx = self.item_idx();
                let state = *self.antenna_blocks.entry(item).or_insert_with(|| {
                    let name = usize::try_from(item).ok().and_then(|i| items_in(&self.source, idx)?.get(i))?;
                    self.source.blocks.iter().find(|b| &b.name == name).map(|b| b.default_state)
                });
                let (Some(h), Some(s)) = (head, state) else { return };
                let c = Vec3::splat(0.5);
                let b = h
                    * Mat4::from_translation(Vec3::new(0.0, -1.75, 0.0))
                    * Mat4::from_translation(Vec3::new(-0.5, 0.0, -0.5))
                    * Mat4::from_translation(c)
                    * Mat4::from_rotation_z(PI)
                    * Mat4::from_translation(-c)
                    * antenna_block_offset(self.holds(e, 7, "pink_petals") || self.holds(e, 7, "wildflowers"));
                self.push_block(s, b, light, 10 << 16);
            }
            // Adult mooshroom: two mushrooms on the back, one on the head (variant 1 brown).
            "mooshroom" if !e.baby() && !invisible => {
                let brown = e.meta_int(18) == Some(1);
                let Some(s) = self.held_blocks[if brown { 3 } else { 2 }] else { return };
                let ov = overlay & 0xffff_0000;
                let finish = Mat4::from_scale(Vec3::new(-1.0, -1.0, 1.0)) * Mat4::from_translation(Vec3::splat(-0.5));
                let back = m * Mat4::from_translation(Vec3::new(0.2, -0.35, 0.5));
                self.push_block(s, back * Mat4::from_rotation_y(deg(-48.0)) * finish, light, ov);
                let second = back
                    * Mat4::from_rotation_y(deg(42.0))
                    * Mat4::from_translation(Vec3::new(0.1, 0.0, -0.6))
                    * Mat4::from_rotation_y(deg(-48.0));
                self.push_block(s, second * finish, light, ov);
                if let Some(h) = head {
                    let t = h * Mat4::from_translation(Vec3::new(0.0, -0.7, -0.2)) * Mat4::from_rotation_y(deg(-78.0));
                    self.push_block(s, t * finish, light, ov);
                }
            }
            _ => {}
        }
    }

    /// Wings worn on the chest (the equipment asset's "wings" layer: elytra) for
    /// avatars, armor stands and undead humanoids or piglins: own wing model,
    /// moved back by 2/16, angles tracked client-side, lowered by 3 px when crouching; player's cape
    /// texture if the asset asks for it and their cape is shown; glint if the stack glows.
    fn push_wings(&mut self, e: &Entity, m: Mat4, pt: f32, baby: bool, light: u32) {
        if !matches!(
            e.kind.as_str(),
            "player"
                | "mannequin"
                | "armor_stand"
                | "zombie"
                | "husk"
                | "drowned"
                | "zombie_villager"
                | "skeleton"
                | "stray"
                | "bogged"
                | "parched"
                | "wither_skeleton"
                | "piglin"
                | "piglin_brute"
                | "zombified_piglin"
        ) {
            return;
        }
        let Some(stack) = &e.equipment[4] else { return };
        let Some(info) = usize::try_from(stack.item).ok().and_then(|i| self.source.items.get(i)) else { return };
        let Some(types) = self.equipment.get(&info.equip_asset) else { return };
        let Some(defs) = types.get("wings") else { return };
        let player_tex = types.contains_key("wings@player");
        let model = if baby { "minecraft:elytra_baby#main" } else { "minecraft:elytra#main" };
        let Some(&layer) = self.layer_index.get(model) else { return };
        let own = if player_tex {
            wings_texture(avatar_extras(&mut self.atlas, &self.custom_capes, &self.custom_elytras, e), shows_cape(e))
        } else {
            None
        };
        let rects: Vec<[f32; 4]> = match own {
            Some(r) => vec![r],
            None => defs.iter().filter_map(|(t, _)| self.atlas.rect(&format!("entity/equipment/wings/{t}"))).collect(),
        };
        if rects.is_empty() {
            return;
        }
        let rot = e.pose_anim.elytra(pt);
        let y = if e.pose() == 5 { 3.0 } else { 0.0 };
        let (parts_buf, vis_buf, mut world_m) = std::mem::take(&mut self.scratch);
        let gl = &self.layers[layer];
        let mut pose = Pose::reset(&gl.baked, parts_buf, vis_buf);
        pose.set("left_wing", |w| {
            w[1] = y;
            w[3] = rot[0];
            w[4] = rot[1];
            w[5] = rot[2];
        });
        pose.set("right_wing", |w| {
            w[1] = y;
            w[3] = rot[0];
            w[4] = -rot[1];
            w[5] = -rot[2];
        });
        let root = m * Mat4::from_translation(Vec3::new(0.0, 0.0, 0.125));
        let lay = if self.gui_root.is_some() { Mat4::IDENTITY } else { self.layering };
        let first = self.mats.len() as u32;
        world_m.clear();
        world_m.resize(gl.baked.parts.len(), None);
        for (i, part) in gl.baked.parts.iter().enumerate() {
            let parent = if part.parent == u32::MAX { Some(root) } else { world_m[part.parent as usize] };
            let pm = parent.filter(|_| pose.visible[i]).map(|p| p * part_matrix(&pose.parts[i]));
            world_m[i] = pm;
            self.mats.push(pm.map_or_else(PartMat::zeroed, |pm| PartMat::new(&(lay * pm))));
        }
        self.scratch = (pose.parts, pose.visible, world_m);
        let (gl_first, gl_count) = (gl.first, gl.count);
        let mut glint = if self.foil(stack) { GLINT_ARMOR } else { 0 };
        for rect in rects {
            self.instances[0].push((
                0,
                gl_first,
                gl_count,
                Instance { base: first, rect, color: 0xffff_ffff, light: light | glint, overlay: 10 << 16 },
            ));
            glint = 0;
        }
    }

    /// The stack glows (enchantment glint: forced component, enchantments, linked compass…).
    fn foil(&self, stack: &rc_proto_v777::item::ItemStack) -> bool {
        let info = usize::try_from(stack.item).ok().and_then(|i| self.source.items.get(i));
        let view =
            crate::stack::StackRef { stack, info, ids: &self.ids, data: &self.source, hold: crate::stack::Hold::NONE };
        rc_assets::item::StackView::glint(&view)
    }

    /// Living entity `e` carrying an item in context `ctx` (needles oriented from it).
    fn owner_env<'a>(&'a self, e: &'a Entity, ctx: &'static str) -> crate::stack::OwnerEnv<'a> {
        let local = e.id == self.objects.local;
        crate::stack::OwnerEnv {
            world: &self.world,
            ctx,
            id: e.id,
            pos: e.pos,
            yaw: e.body,
            living: Some(e.kind.as_str()),
            right_handed: self.right_handed(e),
            local,
            camera: self.camera.map_or(local, |c| c == e.id),
        }
    }

    /// Held items (`entities.md` part 3 § 3.4): at the end of each arm, `thirdperson_*`
    /// context, left-hand transformation mirrored.
    #[allow(clippy::too_many_arguments)]
    fn push_held(
        &mut self,
        e: &Entity,
        models: &rc_assets::item::ItemModels,
        arms: [Option<HandRef>; 2],
        slim: bool,
        eye: Option<(usize, Mat4)>,
        light: u32,
        st: &AnimState,
    ) {
        let right_handed = self.right_handed(e);
        let baby = e.baby();
        // Copper golem at rest (state 0): item turned by a quarter turn.
        let golem_idle = e.meta_int(17).unwrap_or(0) == 0;
        // Shared data (reference count): names borrowed without copying.
        let source = self.source.clone();
        let (mut layers, mut key) = std::mem::take(&mut self.held_scratch);
        for (k, arm) in arms.into_iter().enumerate() {
            let Some(arm) = arm else { continue };
            let right = k == 0;
            let arm = hand_frame(&e.kind, right, &arm, slim, golem_idle);
            // Main hand on the side of the main arm.
            let slot = if right == right_handed { 0 } else { 1 };
            let Some(stack) = &e.equipment[slot] else { continue };
            let info = usize::try_from(stack.item).ok().and_then(|i| source.items.get(i));
            // Fishing player: the line is in the main hand if it holds a fishing rod,
            // otherwise in the other.
            let cast = self.objects.fishers.contains(&e.id) && {
                let rod = self.item_id("minecraft:fishing_rod");
                let main_rod = e.equipment[0].as_ref().is_some_and(|s| Some(s.item) == rod);
                (slot == 0) == main_rod
            };
            let hold = crate::stack::Hold::worn(slot, e.id == self.objects.local, cast);
            let view = crate::stack::StackRef { stack, info, ids: &self.ids, data: &source, hold };
            let Some(model) = view.model() else { continue };
            let name = usize::try_from(stack.item)
                .ok()
                .and_then(|i| items_in(&source, self.item_idx())?.get(i))
                .map_or("", String::as_str);
            // Spyglass in use (no swaying): held at the eye, in the "head" context.
            if let Some((used, head)) = eye
                && used == slot
                && name == "minecraft:spyglass"
            {
                layers.clear();
                key.clear();
                let env = self.owner_env(e, "head");
                models.layers(name, &model, &view, &env, env.seed(), &mut layers, &mut key);
                let at = head * Mat4::from_translation(Vec3::new(if right { 2.5 } else { -2.5 } / 16.0, -0.0625, 0.0));
                for l in &layers {
                    let b = models.baked(l.model).unwrap_or(&models.missing);
                    let d = Mat4::from_cols_array_2d(&b.displays[6]).transpose();
                    self.push_item_layer(models, l, at * d * Mat4::from_cols_array_2d(&l.local).transpose(), light);
                }
                continue;
            }
            let env = self.owner_env(e, if right { "thirdperson_righthand" } else { "thirdperson_lefthand" });
            layers.clear();
            key.clear();
            models.layers(name, &model, &view, &env, env.seed(), &mut layers, &mut key);
            let offset = if baby {
                Vec3::new(0.0, 1.0 / 16.0, -4.5 / 16.0)
            } else {
                Vec3::new(if right { 1.0 } else { -1.0 } / 16.0, 2.0 / 16.0, -10.0 / 16.0)
            };
            let mut hand = arm
                * Mat4::from_rotation_x(deg(-90.0))
                * Mat4::from_rotation_y(deg(180.0))
                * Mat4::from_translation(offset);
            // Spear: lunge of this hand, then use pose (arm's "spear" pose).
            if e.swing.is_some_and(|s| usize::try_from(s.0) == Ok(slot) && s.1 == 2) {
                let fwd = e.equipment[0].as_ref().map_or(0.0, |m| {
                    let info = usize::try_from(m.item).ok().and_then(|i| source.items.get(i));
                    let v = crate::stack::StackRef {
                        stack: m,
                        info,
                        ids: &self.ids,
                        data: &source,
                        hold: crate::stack::Hold::NONE,
                    };
                    crate::spear_anim::forward_movement(&v)
                });
                hand *= crate::spear_anim::attack_item(st.swing, fwd);
            }
            let ticks = if st.using && st.use_right == right { st.use_ticks } else { 0.0 };
            if ticks != 0.0
                && st.arm_pose[usize::from(!right)] == arm::SPEAR
                && let Some(k) = st.spear[usize::from(!right)]
            {
                let fwd = crate::spear_anim::forward_movement(&view);
                // Hit recoil not tracked for the other entities (no item recoil).
                hand *= crate::spear_anim::use_item(k, fwd, ticks, right, st.swing, 100.0);
            }
            for l in &layers {
                let b = models.baked(l.model).unwrap_or(&models.missing);
                let d = Mat4::from_cols_array_2d(&b.displays[if right { 2 } else { 3 }]).transpose();
                let d = if right {
                    d
                } else {
                    // Mirror: tx, ry and rz opposite, centring unchanged.
                    let fx = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
                    fx * (d * Mat4::from_translation(Vec3::splat(0.5))) * fx * Mat4::from_translation(Vec3::splat(-0.5))
                };
                let mtx = hand * d * Mat4::from_cols_array_2d(&l.local).transpose();
                self.push_item_layer(models, l, mtx, light);
            }
        }
        self.held_scratch = (layers, key);
    }

    /// First-person hand, arm (`hand-camera.md` § 2.4 step 4): "arm" part of the
    /// player's skin, default pose turned by ±0.1 rad in Z, sleeve depending on the skin bit,
    /// no flip or entity scale; `m` = hand stack (world frame).
    pub fn push_hand_arm(&mut self, e: &Entity, right: bool, m: Mat4, light: u32) {
        let layers = match self.custom_skins.get(&e.uuid) {
            Some(&i) => i,
            None => self.resolve(e.kind_id, &e.kind, false, u32::from(default_skin(e.uuid))),
        };
        let Some(&r) = self.arena[layers].first() else { return };
        let (parts_buf, vis_buf, mut world_m) = std::mem::take(&mut self.scratch);
        let gl = &self.layers[r.layer];
        let mut pose = Pose::reset(&gl.baked, parts_buf, vis_buf);
        let (arm, sleeve, bit) = if right { ("right_arm", "right_sleeve", 3) } else { ("left_arm", "left_sleeve", 2) };
        let (ai, si) = (pose.index(arm), pose.index(sleeve));
        pose.visible.iter_mut().for_each(|v| *v = false);
        if let Some(i) = ai {
            pose.visible[i] = true;
            let p = &mut pose.parts[i];
            (p[3], p[4], p[5]) = (0.0, 0.0, if right { 0.1 } else { -0.1 });
        }
        // Sleeve: child of the arm, or root part with the same pivot (posed like the arm).
        let sleeve_root = si.is_some_and(|i| gl.baked.parts[i].parent == u32::MAX);
        if let Some(i) = si {
            pose.visible[i] = e.meta_byte(16) as u8 & (1 << bit) != 0;
            if sleeve_root {
                let p = &mut pose.parts[i];
                (p[3], p[4], p[5]) = (0.0, 0.0, if right { 0.1 } else { -0.1 });
            }
        }
        let base = self.mats.len() as u32;
        world_m.clear();
        world_m.resize(gl.baked.parts.len(), None);
        for (i, part) in gl.baked.parts.iter().enumerate() {
            let parent = if Some(i) == ai || (sleeve_root && Some(i) == si) {
                Some(m)
            } else if part.parent == u32::MAX {
                None
            } else {
                world_m[part.parent as usize]
            };
            let pm = parent.filter(|_| pose.visible[i]).map(|p| p * part_matrix(&pose.parts[i]));
            world_m[i] = pm;
            self.mats.push(pm.as_ref().map_or_else(PartMat::zeroed, PartMat::new));
        }
        self.instances[1].push((
            0,
            gl.first,
            gl.count,
            Instance { base, rect: r.rect, color: 0xffff_ffff, light, overlay: 10 << 16 },
        ));
        self.scratch = (pose.parts, pose.visible, world_m);
    }

    /// First-person hand, held item (§ 2.5): `firstperson_*` context depending on the arm,
    /// left-hand transformation mirrored; `m` = hand stack (world frame).
    #[allow(clippy::too_many_arguments)]
    pub fn push_hand_item(
        &mut self,
        models: &rc_assets::item::ItemModels,
        stack: &rc_proto_v777::item::ItemStack,
        right: bool,
        hold: crate::stack::Hold,
        hud: &crate::stack::HudEnv,
        m: Mat4,
        light: u32,
    ) {
        let source = self.source.clone();
        let (mut layers, mut key) = std::mem::take(&mut self.held_scratch);
        let info = usize::try_from(stack.item).ok().and_then(|i| source.items.get(i));
        let view = crate::stack::StackRef { stack, info, ids: &self.ids, data: &source, hold };
        if let Some(model) = view.model() {
            let name = usize::try_from(stack.item)
                .ok()
                .and_then(|i| items_in(&source, self.item_idx())?.get(i))
                .map_or("", String::as_str);
            // Local player carrier: their death location, position, time and keys; seed
            // = their id plus the rank of the context.
            let ctx = if right { "firstperson_righthand" } else { "firstperson_lefthand" };
            let env = crate::stack::InContext(hud, ctx);
            let seed = crate::stack::living_seed(self.objects.local, ctx);
            layers.clear();
            key.clear();
            models.layers(name, &model, &view, &env, seed, &mut layers, &mut key);
            for l in &layers {
                let b = models.baked(l.model).unwrap_or(&models.missing);
                let d = Mat4::from_cols_array_2d(&b.displays[if right { 4 } else { 5 }]).transpose();
                let d = if right {
                    d
                } else {
                    let fx = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
                    fx * (d * Mat4::from_translation(Vec3::splat(0.5))) * fx * Mat4::from_translation(Vec3::splat(-0.5))
                };
                let mtx = m * d * Mat4::from_cols_array_2d(&l.local).transpose();
                self.push_item_layer(models, l, mtx, light | context_glint_bits(Some(if right { 4 } else { 5 })));
            }
        }
        self.held_scratch = (layers, key);
    }

    /// Instances of an item layer (one per tint group) with the given matrix.
    fn push_item_layer(
        &mut self,
        models: &rc_assets::item::ItemModels,
        l: &rc_assets::item::Layer,
        m: Mat4,
        light: u32,
    ) {
        let base_index = self.mats.len() as u32;
        self.mats.push(PartMat::new(&m));
        // Special model (head, banner, shield, chest, Shulker): the layer only carries its
        // data; the geometry comes from the entity layers, in the same transformation.
        // Glint: in the geometry's shader (a single layer), otherwise separate pass over
        // all the layers (patterned shield: glint over the patterns).
        let glint = item_glint_bits(l.glint);
        if l.special != 0 {
            // Entity glint (scale 0.5 on the UV of the model's texture), like the game's trident
            // and shield; the groups carry the rectangle of their texture.
            let glint = if glint != 0 { GLINT_ENTITY } else { 0 };
            let groups = self.special_mesh(l.special, &l.tints);
            let single = groups.len() == 1;
            for (i, g) in groups.iter().enumerate() {
                let inst = Instance {
                    base: base_index,
                    rect: g.rect,
                    color: g.color,
                    light: if single { light | glint } else { light },
                    overlay: 10 << 16,
                };
                // Improved transparency: with the solids, like the game's render types for
                // these models (blended patterns included).
                let pass = special_pass(self.oit, l.special);
                self.instances[pass].push((1, g.first, g.count, inst));
                // Several groups (patterned shield): a single glint pass, on the base
                // model (first group); the patterns are coplanar copies.
                if glint != 0 && !single && i == 0 {
                    self.instances[6].push((1, g.first, g.count, Instance { light: light | glint, ..inst }));
                }
            }
        }
        let groups = self.item_mesh(models, l.model).to_vec();
        for (t, first, count, translucent) in groups {
            let tint = usize::try_from(t).ok().and_then(|t| l.tints.get(t)).copied().unwrap_or(0xffff_ffff);
            let color = (tint >> 16 & 0xff) | (tint & 0xff00) | (tint & 0xff) << 16 | (tint & 0xff00_0000);
            // The game's "cutout item" and "translucent item" types, back faces culled in
            // both cases: faces without translucency cut out (with the solids in improved
            // transparency), the others blended (in the improved-transparency stages).
            let pass = if translucent { PASS_ITEM_TRANSLUCENT } else { PASS_CUTOUT_CULL };
            self.instances[pass].push((
                1,
                first,
                count,
                Instance {
                    base: base_index,
                    rect: [0.0, 0.0, 1.0, 1.0],
                    color,
                    light: light | glint,
                    overlay: 10 << 16,
                },
            ));
        }
    }

    /// Mesh of a special model (first use): groups by tint. For a player head
    /// with a profile, the skin is requested, then used as soon as it has arrived.
    fn special_mesh(&mut self, kind: u16, data: &[u32]) -> Vec<SpecialGroup> {
        // Skin of the head: rectangle in the atlas if it is downloaded.
        let mut skin = None;
        let mut head = None;
        if kind == 2
            && let Some(key) = crate::item_special::head_key_of(data)
        {
            head = Some(key);
            if let Some(&i) = self.custom_skins.get(&key) {
                skin = self.arena.get(i).and_then(|a| a.first()).map(|r| r.rect);
            } else if self.asked_heads.insert(key)
                && let Some(t) = crate::item_special::head_textures(key)
            {
                self.pending_heads.push((key, t));
            }
        }
        let mut cache_key = vec![u32::from(kind), u32::from(skin.is_some())];
        cache_key.extend_from_slice(data);
        if let Some(g) = self.special_meshes.get(&cache_key) {
            return g.clone();
        }
        let mut quads = Vec::new();
        self.special.quads(kind, if head.is_some() { &[] } else { data }, &mut quads);
        if rc_data::env_flag!("RUSTCRAFT_HEADLOG") {
            eprintln!(
                "special model {kind}: {} quads, head {head:x?}, skin {skin:?}, layer {:?}",
                quads.len(),
                self.special.head_texture_size()
            );
        }
        let mut by_tint: Vec<(u32, [f32; 4], Vec<StaticVertex>)> = Vec::new();
        for q in quads {
            let rect = match skin {
                // The quad's UV are normalised to the layer's texture: the skin is read
                // over that same extent in pixels.
                Some(r) => {
                    let [w, h] = self.special.head_texture_size();
                    [r[0], r[1], w, h]
                }
                None => {
                    let Some(name) = self.special.names.get(q.image).cloned() else { continue };
                    let Some(r) = self.atlas.rect(&name) else { continue };
                    r
                }
            };
            let tint = q.tint;
            let color = (tint >> 16 & 0xff) | (tint & 0xff00) | (tint & 0xff) << 16 | (tint & 0xff00_0000);
            // One group per (tint, texture): the instance carries the texture rectangle, the
            // vertex UV remain those of the model (the entity glint uses them).
            let slot = match by_tint.iter().position(|g| g.0 == color && g.1 == rect) {
                Some(i) => i,
                None => {
                    by_tint.push((color, rect, Vec::new()));
                    by_tint.len() - 1
                }
            };
            for k in [0, 1, 2, 0, 2, 3] {
                by_tint[slot].2.push(StaticVertex {
                    pos: q.pos[k].map(|c| c * 16.0),
                    uv: q.uv[k],
                    normal: q.normal,
                    part: 0,
                });
            }
        }
        let mut groups = Vec::new();
        for (color, rect, verts) in by_tint {
            let first = self.item_verts.len() as u32;
            let count = verts.len() as u32;
            self.item_verts.extend(verts);
            groups.push(SpecialGroup { color, rect, first, count });
        }
        if !groups.is_empty() {
            self.item_dirty = true;
        }
        self.special_meshes.insert(cache_key, groups.clone());
        groups
    }

    /// Ground shadow (`entities.md` part 2 § 3.8): tiles on the full blocks under
    /// the entity, alpha depending on the distance to the camera, the height and the light.
    fn push_shadow(&mut self, e: &Entity, pt: f32, origin: [i32; 2], cam: [f64; 3], w: &ShadowWorld<'_>) {
        if !self.has_shadow || !self.shadows {
            return;
        }
        let k = usize::try_from(e.kind_id).unwrap_or(0);
        if k >= self.shadow_kinds.len() {
            self.shadow_kinds.resize(k + 1, None);
        }
        let (mut r, strength) = match objects::display_shadow(e, pt) {
            Some(s) => s,
            None => *self.shadow_kinds[k].get_or_insert_with(|| shadow_of(&e.kind)),
        };
        if r <= 0.0 || e.flags() & 0x20 != 0 {
            return;
        }
        r *= shadow_scale(e);
        let r = r.min(32.0);
        let d2: f64 = (0..3).map(|i| (e.pos[i] - cam[i]).powi(2)).sum();
        let pow = (1.0 - d2 as f32 / 256.0) * strength;
        if pow <= 0.0 {
            return;
        }
        let p = e.render_pos(pt);
        let depth = (pow / 0.5 - 1.0).min(r);
        let (ex, ey, ez) = (p[0], p[1], p[2]);
        let r64 = f64::from(r);
        let center = [(ex - f64::from(origin[0])) as f32, (ez - f64::from(origin[1])) as f32];
        let inv = 1.0 / (2.0 * r);
        for y in (ey - f64::from(depth)).floor() as i32..=ey.floor() as i32 {
            for z in (ez - r64).floor() as i32..=(ez + r64).floor() as i32 {
                for x in (ex - r64).floor() as i32..=(ex + r64).floor() as i32 {
                    if !self.full_cube.get((w.state)(x, y - 1, z) as usize).copied().unwrap_or(false) {
                        continue;
                    }
                    let n = (w.brightness)(x, y, z);
                    if n <= 3 {
                        continue;
                    }
                    let v = n as f32 / 15.0;
                    let lum = w.ambient + (1.0 - w.ambient) * (v / (4.0 - 3.0 * v));
                    let alpha = ((pow - (ey as f32 - y as f32) * 0.5) * 0.5 * lum).clamp(0.0, 1.0);
                    if alpha <= 0.0 {
                        continue;
                    }
                    self.shadow_verts.push(ShadowTile {
                        pos: [(x - origin[0]) as f32, y as f32, (z - origin[1]) as f32],
                        center,
                        inv,
                        alpha,
                    });
                }
            }
        }
    }

    /// Mesh of an item model (first use): groups by tint index, positions in
    /// model pixels, UV in the atlas.
    fn item_mesh(&mut self, models: &rc_assets::item::ItemModels, model: u32) -> &[(i32, u32, u32, bool)] {
        if !self.item_meshes.contains_key(&model) {
            let baked = models.baked(model).unwrap_or(&models.missing);
            let mut tints: Vec<i32> = baked.quads().iter().map(|q| q.tint).collect();
            tints.sort_unstable();
            tints.dedup();
            let mut groups = Vec::new();
            for t in tints {
                let first = self.item_verts.len() as u32;
                let translucent = baked.quads().iter().any(|q| q.tint == t && q.translucent);
                for q in baked.quads().iter().filter(|q| q.tint == t) {
                    let rect = self.atlas.rect(&format!("#item/{}", q.sprite)).unwrap_or([0.0; 4]);
                    let n = q.dir.normal();
                    for k in [0, 1, 2, 0, 2, 3] {
                        self.item_verts.push(StaticVertex {
                            pos: q.pos[k].map(|c| c * 16.0),
                            uv: [rect[0] + q.uv[k][0] * rect[2], rect[1] + q.uv[k][1] * rect[3]],
                            normal: n,
                            part: 0,
                        });
                    }
                }
                groups.push((t, first, self.item_verts.len() as u32 - first, translucent));
            }
            self.item_dirty = true;
            self.item_meshes.insert(model, groups);
        }
        &self.item_meshes[&model]
    }

    /// Ground item (`entities.md` part 3 § 4.1): bobbing, rotation, copies depending on the
    /// quantity, `ground` display context.
    fn push_item(
        &mut self,
        e: &Entity,
        models: &rc_assets::item::ItemModels,
        pt: f32,
        rel: [f32; 3],
        world: [f64; 3],
        light: &dyn Fn(i32, i32, i32) -> (u8, u8),
    ) -> bool {
        let Some(rc_proto_v777::entity::Meta::Item(Some(stack))) = e.meta.get(&8) else { return false };
        let bob_offs = (e.uuid as u32 >> 8) as f32 / (1u32 << 24) as f32 * 2.0 * PI;
        let age = e.age as f32 + pt;
        let bob = (age / 10.0 + bob_offs).sin() * 0.1 + 0.1;
        // Light at the eyes' block (the game's light probe), not at the feet.
        let (b, s) = self.eye_light(e, world, light);
        let block = if e.flags() & 1 != 0 { 15 } else { b };
        let packed = (u32::from(block) * 16) | ((u32::from(s) * 16) << 8);
        // The ground item carries its stack: its position and its own rotation orient the needle.
        let owner = (e.id, e.pos, ground_item_yaw(e.age, bob_offs));
        let pre = Mat4::from_translation(Vec3::from(rel));
        let post = Mat4::from_rotation_y(age / 20.0 + bob_offs);
        self.push_item_cluster(models, stack, owner, pre, Some(bob), post, packed)
    }

    /// Group of items of a stack in the `ground` context (ground item, ominous spawner):
    /// matrix `pre`, rise `lift` (bobbing) plus lift of the bottom of the model's box (+1/16),
    /// neither for `None`, then `post`; copies
    /// depending on the quantity, offset by the game's 48-bit random (seed = item + damage). `owner`:
    /// id, position and visual rotation of the carrier (needles).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_item_cluster(
        &mut self,
        models: &rc_assets::item::ItemModels,
        stack: &rc_proto_v777::item::ItemStack,
        owner: (i32, [f64; 3], f32),
        pre: Mat4,
        lift: Option<f32>,
        post: Mat4,
        packed: u32,
    ) -> bool {
        let info = usize::try_from(stack.item).ok().and_then(|i| self.source.items.get(i));
        let view =
            crate::stack::StackRef { stack, info, ids: &self.ids, data: &self.source, hold: crate::stack::Hold::NONE };
        let Some(model) = view.model() else { return false };
        let name = usize::try_from(stack.item)
            .ok()
            .and_then(|i| self.item_names()?.get(i))
            .cloned()
            .unwrap_or_default();
        let env = crate::stack::OwnerEnv::new(&self.world, "ground", owner.0, owner.1, owner.2);
        let (mut layers, mut key) = (Vec::new(), Vec::new());
        models.layers(&name, &model, &view, &env, env.seed(), &mut layers, &mut key);
        if layers.is_empty() {
            return false;
        }
        // Box of the model after `ground` display and local transformations.
        let ground = |l: &rc_assets::item::Layer| {
            let b = models.baked(l.model).unwrap_or(&models.missing);
            Mat4::from_cols_array_2d(&b.displays[1]).transpose() * Mat4::from_cols_array_2d(&l.local).transpose()
        };
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for l in &layers {
            let m = ground(l);
            for q in models.baked(l.model).unwrap_or(&models.missing).quads() {
                for p in q.pos {
                    let v = m.transform_point3(Vec3::from(p));
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            // Special model: its geometry (the base model is empty) gives the box.
            if l.special != 0 {
                let mut sq = Vec::new();
                self.special.quads(l.special, &l.tints, &mut sq);
                for q in &sq {
                    for p in q.pos {
                        let v = m.transform_point3(Vec3::from(p));
                        lo = lo.min(v);
                        hi = hi.max(v);
                    }
                }
            }
        }
        if lo.x > hi.x {
            return false;
        }
        let n = match stack.count {
            ..=1 => 1,
            2..=16 => 2,
            17..=32 => 3,
            33..=48 => 4,
            _ => 5,
        };
        // 48-bit random of the game, seed = item + damage.
        let damage = rc_assets::item::StackView::damage(&view);
        let mut seed = (i64::from(stack.item) + i64::from(damage)) as u64 ^ 0x5_DEEC_E66D;
        let mut rand = move || {
            seed = seed.wrapping_mul(0x5_DEEC_E66D).wrapping_add(11) & ((1 << 48) - 1);
            (seed >> 24) as f32 / (1u32 << 24) as f32
        };
        let depth = hi.z - lo.z;
        let base = match lift {
            Some(bob) => pre * Mat4::from_translation(Vec3::new(0.0, bob + (-lo.y + 0.0625), 0.0)) * post,
            None => pre * post,
        };
        let mut copies = Vec::with_capacity(n);
        if depth > 0.0625 {
            copies.push(base);
            for _ in 1..n {
                let o =
                    Vec3::new((rand() * 2.0 - 1.0) * 0.15, (rand() * 2.0 - 1.0) * 0.15, (rand() * 2.0 - 1.0) * 0.15);
                copies.push(base * Mat4::from_translation(o));
            }
        } else {
            let step = depth * 1.5;
            let mut z = -step * (n as f32 - 1.0) / 2.0;
            copies.push(base * Mat4::from_translation(Vec3::new(0.0, 0.0, z)));
            for _ in 1..n {
                z += step;
                let o = Vec3::new((rand() * 2.0 - 1.0) * 0.075, (rand() * 2.0 - 1.0) * 0.075, z);
                copies.push(base * Mat4::from_translation(o));
            }
        }
        for c in copies {
            for l in &layers {
                self.push_item_layer(models, l, c * ground(l), packed);
            }
        }
        true
    }

    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        // Glint scroll (speed and strength settings).
        let (speed, strength) = self.glint;
        let (o0, o1) = glint_offsets(self.glint_started.elapsed().as_secs_f64() * 1000.0, speed);
        queue.write_buffer(&self.lights, 64, bytemuck::cast_slice(&[o0, o1, strength, 0.0]));
        queue.write_buffer(&self.lights_totem, 64, bytemuck::cast_slice(&[o0, o1, strength, 0.0]));
        queue.write_buffer(&self.lights_ui, 64, bytemuck::cast_slice(&[o0, o1, strength, 0.0]));
        self.batches.clear();
        self.glow_batches.clear();
        self.shadow_count = self.shadow_verts.len() as u32;
        if !self.shadow_verts.is_empty() {
            let bytes: &[u8] = bytemuck::cast_slice(&self.shadow_verts);
            if self.shadow_buf.as_ref().is_none_or(|(_, cap)| *cap < bytes.len() as u64) {
                let cap = (bytes.len() as u64).next_power_of_two().max(256);
                self.shadow_buf = Some((
                    device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("entity shadows"),
                        size: cap,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }),
                    cap,
                ));
            }
            queue.write_buffer(&self.shadow_buf.as_ref().expect("created above").0, 0, bytes);
        }
        let free: Vec<FreeVertex> = self.free_verts.concat();
        if !free.is_empty() {
            let bytes: &[u8] = bytemuck::cast_slice(&free);
            if self.free_buf.as_ref().is_none_or(|(_, cap)| *cap < bytes.len() as u64) {
                let cap = (bytes.len() as u64).next_power_of_two().max(256);
                self.free_buf = Some((
                    device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("entity free geometry"),
                        size: cap,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }),
                    cap,
                ));
            }
            queue.write_buffer(&self.free_buf.as_ref().expect("created above").0, 0, bytes);
        }
        if self.mats.is_empty() {
            return;
        }
        // Cutouts and eyes grouped by layer rank then by geometry (stable sort: at equal rank,
        // the placement order is kept); translucents from farthest to nearest, batches of
        // identical consecutive layers. Real source = low 4 bits.
        // World first, then the hand (separate batches, drawn in its pass).
        let from = self.hand_from;
        for p in [0, 2, PASS_CUTOUT_CULL, PASS_SOLID] {
            let f = from[p].min(self.instances[p].len());
            self.instances[p][..f].sort_by_key(|i| (i.0 >> 4, i.0 & 15, i.1));
        }
        // Translucent items and moving blocks of the world: from farthest to nearest according to their
        // pose (stable sort: at equal distance, placement order).
        let f = from[PASS_ITEM_TRANSLUCENT].min(self.instances[PASS_ITEM_TRANSLUCENT].len());
        sort_far_to_near(&mut self.instances[PASS_ITEM_TRANSLUCENT][..f], &self.mats, self.eye);
        let mut flat: Vec<Instance> = Vec::new();
        // World then hand; dissolve (opaque) just after the cutouts.
        // Segments: world, hand, totem animation, interface models.
        let gui = self.gui_from;
        let totem = self.totem_from;
        for seg in 0..4 {
            match seg {
                1 => self.hand_batch = self.batches.len(),
                2 => self.totem_batch = self.batches.len(),
                3 => self.gui_batch = self.batches.len(),
                _ => {}
            }
            let seg_first = self.batches.len();
            for pass in PASS_ORDER {
                let list = &self.instances[pass];
                let a = from[pass].min(list.len());
                let g = gui.map_or(list.len(), |g| g[pass].min(list.len())).max(a);
                let t = totem.map_or(g, |t| t[pass].min(g)).max(a);
                let range = match seg {
                    0 => 0..a,
                    1 => a..t,
                    2 => t..g,
                    _ => g..list.len(),
                };
                for &(src, first, count, inst) in &list[range] {
                    let src = src & 15;
                    let n = flat.len() as u32;
                    let open = self.batches.len() > seg_first;
                    match self.batches.last_mut() {
                        Some(b) if open && b.0 == pass && b.1 == src && b.2 == (first..first + count) => b.4 += 1,
                        _ => self.batches.push((pass, src, first..first + count, n, 1)),
                    }
                    flat.push(inst);
                }
            }
        }
        // Silhouettes of the glowing entities: after the frame's instances.
        for &(src, first, count, inst) in &self.glow_list {
            let src = src & 15;
            let n = flat.len() as u32;
            match self.glow_batches.last_mut() {
                Some(b) if b.0 == src && b.1 == (first..first + count) => b.3 += 1,
                _ => self.glow_batches.push((src, first..first + count, n, 1)),
            }
            flat.push(inst);
        }
        if self.item_dirty && !self.item_verts.is_empty() {
            self.item_dirty = false;
            use wgpu::util::DeviceExt;
            self.item_buf = Some((
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("item meshes"),
                    contents: bytemuck::cast_slice(&self.item_verts),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
                0,
            ));
        }
        let grow = |buf: &mut Option<(wgpu::Buffer, u64)>, bytes: &[u8], usage: wgpu::BufferUsages, label| {
            let fresh = buf.as_ref().is_none_or(|(_, cap)| *cap < bytes.len() as u64);
            if fresh {
                let cap = (bytes.len() as u64).next_power_of_two().max(256);
                *buf = Some((
                    device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some(label),
                        size: cap,
                        usage: usage | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }),
                    cap,
                ));
            }
            queue.write_buffer(&buf.as_ref().expect("created above").0, 0, bytes);
            fresh
        };
        let new_mats =
            grow(&mut self.mats_buf, bytemuck::cast_slice(&self.mats), wgpu::BufferUsages::STORAGE, "entity parts");
        grow(&mut self.inst_buf, bytemuck::cast_slice(&flat), wgpu::BufferUsages::VERTEX, "entity instances");
        let atlas_changed = self.atlas.flush(device, queue);
        if new_mats || atlas_changed || self.bind.is_none() {
            let make = |lights: &wgpu::Buffer| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("entities"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(self.atlas.view().expect("uploaded above")),
                        },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                        wgpu::BindGroupEntry { binding: 2, resource: lights.as_entire_binding() },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: self.mats_buf.as_ref().expect("created above").0.as_entire_binding(),
                        },
                    ],
                })
            };
            let (bind, bind_totem) = (make(&self.lights), make(&self.lights_totem));
            self.bind = Some(bind);
            self.bind_totem = Some(bind_totem);
            self.bind_ui = Some(make(&self.lights_ui));
        }
    }

    /// Drawing in the frame pass (group 0 = camera already bound).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.draw_batches(pass, 0);
    }

    /// First-person hand (group 0 = hand camera already bound).
    pub fn draw_hand(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.draw_batches(pass, 1);
    }

    /// Totem animation (group 0 = camera of the view without rotation already bound).
    pub fn draw_totem(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.draw_batches(pass, 2);
    }

    /// Interface models (group 0 = interface camera already bound).
    pub fn draw_gui(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.draw_batches(pass, 3);
    }

    /// Start of the totem animation: what is added next (up to the interface models) is
    /// drawn by [`Self::draw_totem`], with the fixed lights of the items.
    pub fn begin_totem(&mut self) {
        self.totem_from = Some(std::array::from_fn(|i| self.instances[i].len()));
    }

    /// Start of the interface models: what is added next, after the hand, is drawn
    /// by [`Self::draw_gui`].
    pub fn begin_gui(&mut self) {
        self.gui_from = Some(std::array::from_fn(|i| self.instances[i].len()));
    }

    /// Entity drawn in the interface with root matrix `root` (interface frame),
    /// full light.
    pub fn push_gui_entity(&mut self, e: &Entity, root: Mat4, items: Option<&rc_assets::item::ItemModels>) {
        let family = animals::skin(self, e)
            .or_else(|| creatures::skin(self, e))
            .or_else(|| dragon::skin(self, e))
            .or_else(|| wither::skin(self, e))
            .or_else(|| shulker::skin(self, e));
        let skin = match e.kind.as_str() {
            _ if family.is_some() => family.unwrap_or_default(),
            "player" | "mannequin" => u32::from(avatar_default_skin(e)),
            "armor_stand" => u32::from(e.meta_byte(15) & 1 != 0),
            _ => self.own_skin(e, None),
        };
        let layers = match self.custom_skins.get(&e.uuid) {
            Some(&i) if e.kind == "player" => i,
            _ => {
                let base = self.resolve(e.kind_id, &e.kind, e.baby(), skin);
                if e.kind == "mannequin" { self.mannequin_layers(e, base) } else { base }
            }
        };
        if self.arena[layers].is_empty() {
            return;
        }
        self.gui_root = Some(root);
        self.push_entity(e, layers, 1.0, [0.0; 3], [0.0; 3], &|_, _, _| (15, 15), items, PoseEnv::default());
        self.gui_root = None;
    }

    /// Enchanting book in the interface (`special-screens.md` § 3.7).
    pub fn push_gui_book(&mut self, root: Mat4, open: f32, f1: f32, f2: f32) {
        self.push_book(root, 0xf0f0, 0.0, f1, f2, open);
    }

    fn draw_batches(&self, pass: &mut wgpu::RenderPass<'_>, seg: usize) {
        let (Some(bind), Some((inst, _))) = (&self.bind, &self.inst_buf) else { return };
        let len = self.batches.len();
        let split = self.hand_batch.min(len);
        let gui = if self.gui_from.is_some() { self.gui_batch.clamp(split, len) } else { len };
        let totem = if self.totem_from.is_some() { self.totem_batch.clamp(split, gui) } else { gui };
        let hand = seg != 0;
        let batches = match seg {
            0 => &self.batches[..split],
            1 => &self.batches[split..totem],
            2 => &self.batches[totem..gui],
            _ => &self.batches[gui..],
        };
        let bind = match (seg, &self.bind_totem, &self.bind_ui) {
            (2, Some(b), _) | (3, _, Some(b)) => b,
            _ => bind,
        };
        if batches.is_empty() && (hand || self.shadow_count == 0) {
            return;
        }
        pass.set_bind_group(1, bind, &[]);
        // Shadows first (first of the game's translucent phase).
        if !hand
            && self.shadow_count > 0
            && let Some((sb, _)) = &self.shadow_buf
        {
            pass.set_pipeline(&self.shadow);
            pass.set_vertex_buffer(0, sb.slice(..));
            pass.draw(0..6, 0..self.shadow_count);
        }
        let beams = self.free_verts[FREE_BEAM].len() as u32;
        let rays = self.free_verts[FREE_RAYS].len() as u32;
        let gbeams = self.free_verts[FREE_GBEAM].len() as u32;
        let g_range = beams + rays..beams + rays + gbeams;
        let free_buf = self.free_buf.as_ref().map(|(b, _)| b).filter(|_| !hand && beams + rays + gbeams > 0);
        let mut beams_done = beams + gbeams == 0;
        pass.set_vertex_buffer(1, inst.slice(..));
        let (mut current, mut bound) = (usize::MAX, u8::MAX);
        for (p, src, verts, first, count) in batches {
            // Beams (opaque) after the cutouts and the dissolve, in the world pass.
            if !hand
                && !beams_done
                && !matches!(*p, 0 | 5 | PASS_CUTOUT_CULL | PASS_SOLID)
                && let Some(fb) = free_buf
            {
                beams_done = true;
                pass.set_vertex_buffer(0, fb.slice(..));
                pass.set_pipeline(&self.free_beam);
                pass.draw(0..beams, 0..1);
                pass.set_pipeline(&self.free_gbeam);
                pass.draw(g_range.clone(), 0..1);
                pass.set_vertex_buffer(1, inst.slice(..));
                (current, bound) = (usize::MAX, u8::MAX);
            }
            if *p != current {
                pass.set_pipeline(
                    [
                        &self.cutout,
                        &self.translucent,
                        &self.eyes,
                        &self.water_mask,
                        &self.additive,
                        &self.dissolve,
                        &self.glint_pass,
                        &self.swirl,
                        &self.cutout_cull,
                        &self.solid,
                        &self.translucent_cull,
                        &self.translucent,
                        &self.translucent_cull,
                    ][*p],
                );
                current = *p;
            }
            if *src != bound {
                match src {
                    0 => pass.set_vertex_buffer(0, self.statics.slice(..)),
                    _ => match &self.item_buf {
                        Some((b, _)) => pass.set_vertex_buffer(0, b.slice(..)),
                        None => continue,
                    },
                }
                bound = *src;
            }
            pass.draw(verts.clone(), *first..*first + *count);
        }
        if let Some(fb) = free_buf {
            pass.set_vertex_buffer(0, fb.slice(..));
            if !beams_done {
                pass.set_pipeline(&self.free_beam);
                pass.draw(0..beams, 0..1);
                pass.set_pipeline(&self.free_gbeam);
                pass.draw(g_range, 0..1);
            }
            if rays > 0 {
                pass.set_pipeline(&self.free_rays);
                pass.draw(beams..beams + rays, 0..1);
            }
        }
    }
}

/// Sorts translucent item or block instances from farthest to nearest from `eye`:
/// distance from the pose origin (item) or the cube centre (block, [`SRC_BLOCK_CENTER`]),
/// placement order kept at equal distance.
fn sort_far_to_near(list: &mut [(u8, u32, u32, Instance)], mats: &[PartMat], eye: Vec3) {
    let dist = |&(src, _, _, inst): &(u8, u32, u32, Instance)| {
        let Some(p) = mats.get(inst.base as usize) else { return 0.0 };
        let m = Mat4::from_cols_array_2d(&p.m);
        let at = if src & SRC_BLOCK_CENTER != 0 { Vec3::splat(0.5) } else { Vec3::ZERO };
        m.transform_point3(at).distance_squared(eye)
    };
    list.sort_by(|a, b| dist(b).total_cmp(&dist(a)));
}

/// Animation of a layer depending on the entity kind (`entities.md` part 3 § 2).
/// Alpha of the glowing layers: warden (0 bioluminescence 1; 1 and 2 pulsing spots
/// max(0, cos(age × 0.045 [+ π]) × 0.25); 3 tentacles; 4 heart, beating every
/// 40 − ⌊anger/80 × 30⌋ ticks for 10 ticks), 5 eyes of the active creaking.
fn pulse_alpha(e: &Entity, id: u8, pt: f32) -> f32 {
    let age = e.age as f32 + pt;
    match id {
        0 => 1.0,
        1 => ((age * 0.045).cos() * 0.25).max(0.0),
        2 => ((age * 0.045 + PI).cos() * 0.25).max(0.0),
        3 => (e.extra[2] + pt * (e.extra[1] - e.extra[2])) / 10.0,
        4 => {
            let anger = (e.meta_int(16).unwrap_or(0) as f32 / 80.0).clamp(0.0, 1.0);
            let delay = 40 - (anger * 30.0).floor() as u32;
            // Beat at the tick multiple of the delay: 10 then countdown (after the tick: 9).
            let k = e.age % delay;
            let (now, before) = ((9i32 - k as i32).max(0) as f32, (10i32 - k as i32).max(0) as f32);
            (before + pt * (now - before)) / 10.0
        }
        5 => f32::from(u8::from(e.meta_bool(17))),
        _ => 1.0,
    }
}

/// Inputs of a living entity's pose rotations.
struct RotCtx {
    shaking: bool,
    upside_down: bool,
    fall_flying: bool,
    swim: f32,
    env: PoseEnv,
    /// Current height of the box (scale attribute included).
    height: f32,
}

/// Rotations of a living entity after its scaling: shaking, body yaw (or those
/// of its family), death tilt, spin attack, bed, upside down, then glide and
/// swim of the avatars, drowned swim. `body`: interpolated body yaw (degrees).
fn pose_rotations(e: &Entity, pt: f32, body: f32, m: &mut Mat4, c: &RotCtx) {
    // Renders that replace the base rotations (squids, armor stand, dragon):
    // no shaking, no death tilt, no poses (bed, Riptide, upside down).
    let base_rot = !matches!(e.kind.as_str(), "squid" | "glow_squid" | "ender_dragon" | "armor_stand");
    // Shaking: body yaw offset; Y rotation applied before that of the body
    // (coincident axes, the families' translations are vertical).
    let shake = if base_rot && c.shaking { ((e.age as f32 + pt).floor() * 3.25).cos() * PI * 0.4 } else { 0.0 };
    let sleeping = base_rot && e.pose() == 2;
    if !sleeping {
        *m *= Mat4::from_rotation_y(deg(-shake));
    }
    let family = !sleeping
        && (animals::rotations(e, pt, m)
            || creatures::rotations(e, pt, m)
            || dragon::rotations(e, pt, m)
            || shulker::rotations(e, pt, m));
    if !family && !sleeping {
        *m *= Mat4::from_rotation_y(deg(180.0 - body));
    }
    // Armor stand hit (event 32): 5-tick oscillation around Y.
    if e.kind == "armor_stand" && e.timers[0] > 0 {
        let w = (5 - e.timers[0]) as f32 + pt;
        *m *= Mat4::from_rotation_y(deg((w / 1.5 * std::f32::consts::PI).sin() * 3.0));
    }
    // Pitch of the entity in degrees (inverted upside down).
    let x_rot = (e.pitch_o + pt * (e.pitch - e.pitch_o)) * if c.upside_down { -1.0 } else { 1.0 };
    let flip =
        if matches!(e.kind.as_str(), "spider" | "cave_spider" | "endermite" | "silverfish") { 180.0 } else { 90.0 };
    let spin = e.meta_byte(8) & 4 != 0;
    if !base_rot {
    } else if e.death_time > 0 {
        // Death tilt.
        let t = e.death_time as f32 - 1.0 + pt;
        let f = (t / 20.0 * 1.6).sqrt().min(1.0);
        *m *= Mat4::from_rotation_z(deg(f * flip));
    } else if spin {
        // Spin attack (Riptide): lying along the gaze axis, spinning top.
        *m *= Mat4::from_rotation_x(deg(-90.0 - x_rot)) * Mat4::from_rotation_y(deg((e.age as f32 + pt) * -75.0));
    } else if sleeping {
        // Lying: head towards the head of the bed (bed direction), otherwise body yaw.
        let angle = c.env.bed.map_or(body + shake, |b| b.2);
        *m *= Mat4::from_rotation_y(deg(angle)) * Mat4::from_rotation_z(deg(flip)) * Mat4::from_rotation_y(deg(270.0));
    } else if c.upside_down {
        let h = c.height;
        let s = living_scale(e);
        *m *= Mat4::from_translation(Vec3::new(0.0, (h + 0.1) / s, 0.0)) * Mat4::from_rotation_z(PI);
    }
    if matches!(e.kind.as_str(), "player" | "mannequin") {
        let pa = &e.pose_anim;
        if c.fall_flying {
            // Glide: lying forward as the flight sets in, then
            // turned towards the movement direction.
            if !spin {
                *m *= Mat4::from_rotation_x(deg(pa.fly_scale(pt) * (-90.0 - x_rot)));
            }
            if let Some(a) = flying_y_rot(e, pt) {
                *m *= Mat4::from_rotation_y(a);
            }
        } else if c.swim > 0.0 {
            let target = if c.env.in_water { -90.0 - x_rot } else { -90.0 };
            *m *= Mat4::from_rotation_x(deg(c.swim * target));
            if visually_swimming(e) {
                *m *= Mat4::from_translation(Vec3::new(0.0, -1.0, 0.3));
            }
        }
    }
    // Swimming drowned: tilted around the middle of its box.
    if e.kind == "drowned" && c.swim > 0.0 {
        let pivot = Vec3::new(0.0, c.height / 2.0 / living_scale(e), 0.0);
        *m *= Mat4::from_translation(pivot)
            * Mat4::from_rotation_x(deg(c.swim * (-10.0 - x_rot)))
            * Mat4::from_translation(-pivot);
    }
}

fn animate(pose: &mut Pose<'_>, e: &Entity, state: &AnimState, pt: f32, living: bool) {
    if animals::animate(pose, e, state, pt)
        || creatures::animate(pose, e, state, pt)
        || dragon::animate(pose, e, state, pt)
        || wither::animate(pose, e, state, pt)
        || shulker::animate(pose, e, state, pt)
    {
        return;
    }
    match e.kind.as_str() {
        "zombie" => {
            humanoid(pose, state);
            let aggressive = e.meta_byte(15) & 4 != 0;
            zombie_arms(pose, state, aggressive, !state.baby || e.equipment[0].is_none());
        }
        "player" | "mannequin" => {
            humanoid(pose, state);
            // Visible skin parts (index 16, player as mannequin): jacket, sleeves,
            // trousers, hat.
            let skin = e.meta_byte(16) as u8;
            for (bit, part) in [
                (1, "jacket"),
                (2, "left_sleeve"),
                (3, "right_sleeve"),
                (4, "left_pants"),
                (5, "right_pants"),
                (6, "hat"),
            ] {
                if skin & (1 << bit) == 0 {
                    pose.hide(part);
                }
            }
        }
        "cow" | "pig" | "sheep" | "creeper" | "mooshroom" | "polar_bear" | "goat" => {
            quadruped_legs(pose, state, e.kind == "creeper");
            if e.kind == "sheep" && e.graze > 0 {
                let c = e.graze as f32;
                let ph = if (4.0..=36.0).contains(&c) {
                    1.0
                } else if c < 4.0 {
                    (c - pt) / 4.0
                } else {
                    -(c - 40.0 - pt) / 4.0
                };
                let ah = if c > 4.0 && c <= 36.0 {
                    PI / 5.0 + 0.219_911_49 * ((c - 4.0 - pt) / 32.0 * 28.7).sin()
                } else {
                    PI / 5.0
                };
                let age_scale = if state.baby { 0.5 } else { 1.0 };
                pose.set("head", |h| {
                    h[1] += ph * 9.0 * age_scale;
                    h[3] = ah;
                });
            }
        }
        "spider" | "cave_spider" => spider(pose, state),
        "husk" | "zombie_villager" => {
            humanoid(pose, state);
            zombie_arms(pose, state, e.meta_byte(15) & 4 != 0, !state.baby || e.equipment[0].is_none());
        }
        // Drowned: own swim in place of the breaststroke.
        "drowned" => {
            humanoid(pose, &AnimState { swim: 0.0, ..*state });
            zombie_arms(pose, state, e.meta_byte(15) & 4 != 0, !state.baby || e.equipment[0].is_none());
            if state.swim > 0.0 {
                drowned_swim(pose, state);
            }
        }
        "giant" => {
            humanoid(pose, state);
            zombie_arms(pose, state, false, true);
        }
        "skeleton" | "stray" | "bogged" | "parched" | "wither_skeleton" => {
            humanoid(pose, state);
            skeleton_arms(pose, state, e.meta_byte(15) & 4 != 0);
            if e.kind == "bogged" && e.meta_bool(16) {
                pose.hide("mushrooms");
            }
        }
        "zombified_piglin" => {
            humanoid(pose, state);
            piglin_ears(pose, state);
            zombie_arms(pose, state, e.meta_byte(15) & 4 != 0, true);
        }
        "piglin" | "piglin_brute" => piglin(pose, e, state),
        "pillager" | "vindicator" | "evoker" | "illusioner" => illager(pose, e, state),
        "vex" => vex(pose, e, state),
        "ravager" => ravager(pose, e, state, pt),
        "zoglin" | "hoglin" => hoglin(pose, e, state),
        "armor_stand" => armor_stand(pose, e),
        "evoker_fangs" => fangs(pose, e, pt),
        "chicken" => {
            let q = state.walk_pos * 0.6662;
            pose.set("right_leg", |l| l[3] = q.cos() * 1.4 * state.walk_speed);
            pose.set("left_leg", |l| l[3] = (q + PI).cos() * 1.4 * state.walk_speed);
            if !state.baby {
                pose.set("head", |h| {
                    h[3] = state.pitch;
                    h[4] = state.yaw;
                });
            }
            let f = e.flap[0] + pt * (e.flap[1] - e.flap[0]);
            let fs = e.flap[2] + pt * (e.flap[3] - e.flap[2]);
            let angle = (f.sin() + 1.0) * fs;
            pose.set("right_wing", |w| w[5] = angle);
            pose.set("left_wing", |w| w[5] = -angle);
        }
        _ if living => generic(pose, state),
        _ => {}
    }
}

/// Animal equipment layers (saddle, horse or nautilus armor, llama carpet,
/// ghast harness): the game never tests invisibility for them (floating saddle).
fn equipment_layer(name: &str) -> bool {
    name.ends_with("#saddle") || name.ends_with("#decor") || name.contains("_armor#") || name.contains("_harness#")
}

/// Fire-immune kinds (no flames drawn), and the falling block.
const FLAME_IMMUNE: [&str; 15] = [
    "area_effect_cloud",
    "ender_dragon",
    "end_crystal",
    "ghast",
    "magma_cube",
    "shulker",
    "strider",
    "tnt",
    "vex",
    "warden",
    "wither",
    "wither_skeleton",
    "zoglin",
    "zombified_piglin",
    "falling_block",
];

/// Flags of `KindInfo` (one bit per kind predicate read every frame).
const K_BIG_BOX: u32 = 1;
const K_ILLUSIONER: u32 = 1 << 1;
const K_DISPLAY: u32 = 1 << 2;
const K_ALWAYS: u32 = 1 << 3;
const K_END_CRYSTAL: u32 = 1 << 4;
const K_ENDER_DRAGON: u32 = 1 << 5;
pub(super) const K_OBJECT: u32 = 1 << 6;
const K_PAINTING: u32 = 1 << 7;
const K_ORB: u32 = 1 << 8;
const K_ITEM: u32 = 1 << 9;
const K_FLAME_IMMUNE: u32 = 1 << 10;
const K_BLAZE: u32 = 1 << 11;
const K_AVATAR: u32 = 1 << 12;
const K_PLAYER: u32 = 1 << 13;
const K_MANNEQUIN: u32 = 1 << 14;
const K_SPEAR_MOB: u32 = 1 << 15;
const K_SKELETON: u32 = 1 << 16;
const K_DROWNED: u32 = 1 << 17;
const K_PIGLIN: u32 = 1 << 18;
const K_SELF_LIT: u32 = 1 << 19;
const K_GLOW_SQUID: u32 = 1 << 20;

/// What the render asks of an entity's kind every frame, resolved once per kind
/// (table by type id): flags `K_*`, family whose skin function answers
/// (0 none, 1 animals, 2 mobs, 3 dragon, 4 wither, 5 shulker) and serializer of the
/// variant read by `own_skin` (0 without).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) struct KindInfo {
    pub flags: u32,
    pub family: u8,
    pub variant_ser: u8,
}

impl KindInfo {
    pub(super) fn has(self, f: u32) -> bool {
        self.flags & f != 0
    }

    /// Definition: the same string predicates as the old path.
    fn of(k: &str) -> KindInfo {
        let mut flags = 0;
        let mut set = |on: bool, f: u32| {
            if on {
                flags |= f;
            }
        };
        set(matches!(k, "ender_dragon" | "ghast" | "happy_ghast" | "giant" | "wither"), K_BIG_BOX);
        set(k == "illusioner", K_ILLUSIONER);
        set(matches!(k, "block_display" | "item_display" | "text_display"), K_DISPLAY);
        set(matches!(k, "lightning_bolt" | "fishing_bobber" | "ender_dragon"), K_ALWAYS);
        set(k == "end_crystal", K_END_CRYSTAL);
        set(k == "ender_dragon", K_ENDER_DRAGON);
        set(objects::object_kind(k), K_OBJECT);
        set(k == "painting", K_PAINTING);
        set(k == "experience_orb", K_ORB);
        set(k == "item", K_ITEM);
        set(FLAME_IMMUNE.contains(&k), K_FLAME_IMMUNE);
        set(k == "blaze", K_BLAZE);
        set(matches!(k, "player" | "mannequin"), K_AVATAR);
        set(k == "player", K_PLAYER);
        set(k == "mannequin", K_MANNEQUIN);
        set(
            matches!(k, "zombie" | "husk" | "zombie_villager" | "drowned" | "zombified_piglin") || is_skeleton(k),
            K_SPEAR_MOB,
        );
        set(is_skeleton(k), K_SKELETON);
        set(k == "drowned", K_DROWNED);
        set(matches!(k, "piglin" | "piglin_brute"), K_PIGLIN);
        set(matches!(k, "vex" | "magma_cube" | "wither" | "blaze" | "allay"), K_SELF_LIT);
        set(k == "glow_squid", K_GLOW_SQUID);
        KindInfo { flags, family: family_of(k), variant_ser: own_variant_ser(k) }
    }
}

/// First family whose skin function answers for this kind (order of `prepare`): each
/// function answers by kind alone (`kind_table_matches_predicates` checks it).
fn family_of(k: &str) -> u8 {
    if animals::ours(k) {
        1
    } else if creatures::ours(k) {
        2
    } else if k == "ender_dragon" {
        3
    } else if k == "wither" {
        4
    } else if k == "shulker" {
        5
    } else {
        0
    }
}

/// Serializer of the variant of the animals handled by `own_skin` (0: no variant).
fn own_variant_ser(k: &str) -> u8 {
    match k {
        "cow" => 23,
        "pig" => 28,
        "chicken" => 30,
        "cat" => 21,
        "frog" => 27,
        "wolf" => 25,
        _ => 0,
    }
}

/// Flags of an item (id of the `minecraft:item` registry) read every frame.
const I_CROSSBOW: u8 = 1;
const I_BOW: u8 = 1 << 1;
const I_TRIDENT: u8 = 1 << 2;
const I_SPEAR: u8 = 1 << 3;

fn item_bits(name: &str) -> u8 {
    let short = name.strip_prefix("minecraft:");
    let mut b = 0;
    if short == Some("crossbow") {
        b |= I_CROSSBOW;
    }
    if short == Some("bow") {
        b |= I_BOW;
    }
    if short == Some("trident") {
        b |= I_TRIDENT;
    }
    if name.ends_with("_spear") {
        b |= I_SPEAR;
    }
    b
}

/// Item flag `I_*` matching the short name looked up by `holds`, 0 if none.
fn item_bit_of(name: &str) -> u8 {
    match name {
        "crossbow" => I_CROSSBOW,
        "bow" => I_BOW,
        "trident" => I_TRIDENT,
        _ => 0,
    }
}

impl EntityRenderer {
    /// Flags of the kind of `e`: table by type id (name checked), otherwise computed
    /// from the name (kind changed by hand, unknown id).
    pub(super) fn kind_of(&self, e: &Entity) -> KindInfo {
        match usize::try_from(e.kind_id).ok().and_then(|i| self.kinds.get(i)) {
            Some((n, info)) if *n == e.kind => *info,
            _ => KindInfo::of(&e.kind),
        }
    }

    /// The entity currently in `prepare` (its sizes and probe hold for it alone; the
    /// entities do not change during `prepare`).
    fn is_current(&self, e: &Entity) -> bool {
        self.cur_addr != 0 && self.cur_addr == std::ptr::from_ref(e) as usize
    }

    /// Enters entity `e` of `prepare` (`None`: exit), sizes (base, full) already
    /// computed if `SIZEONCE`.
    fn set_current(&mut self, e: Option<&Entity>, sizes: Option<Sizes>) {
        self.cur_addr = e.map_or(0, |e| std::ptr::from_ref(e) as usize);
        self.cur_light.set(None);
        self.cur_size = sizes.filter(|_| e.is_some());
    }

    /// Height, eyes and label without scale (`Entity::dims`), kept for the current entity.
    pub(super) fn dims_of(&self, e: &Entity) -> [f32; 3] {
        match self.cur_size {
            Some((b, _)) if self.is_current(e) => [b.h, b.eye, b.tag],
            _ => e.dims(&self.dims),
        }
    }

    /// Full size (`Entity::size`), kept for the current entity.
    pub(super) fn size_of(&self, e: &Entity) -> Size {
        match self.cur_size {
            Some((_, s)) if self.is_current(e) => s,
            _ => e.size(&self.dims),
        }
    }

    /// Raw light probe (block, sky) at the eyes' block of `e` placed at `world` (eye height
    /// without scale), done once for the entity currently in `prepare` and this point.
    pub(super) fn eye_light(&self, e: &Entity, world: [f64; 3], light: &dyn Fn(i32, i32, i32) -> (u8, u8)) -> (u8, u8) {
        let keep = self.opt.light_once && self.is_current(e);
        let key = world.map(f64::to_bits);
        if keep && let Some((k, v)) = self.cur_light.get() && k == key {
            return v;
        }
        let eye_y = world[1] + f64::from(self.dims_of(e)[1]);
        let v = light(world[0].floor() as i32, eye_y.floor() as i32, world[2].floor() as i32);
        if keep {
            self.cur_light.set(Some((key, v)));
        }
        v
    }

    /// Names of the item registry.
    pub(super) fn item_names(&self) -> Option<&[String]> {
        items_in(&self.source, self.item_idx())
    }

    /// Kept index of the item registry (`REGIDX`), `None`: search by name.
    pub(super) fn item_idx(&self) -> Option<usize> {
        self.item_reg.filter(|_| self.opt.reg_idx)
    }
}

/// Names of the item registry of `source`: registry of index `idx` kept, otherwise looked up by name.
pub(super) fn items_in(source: &VersionData, idx: Option<usize>) -> Option<&[String]> {
    match idx {
        Some(i) => source.registries.get(i),
        None => source.registry("minecraft:item"),
    }
    .map(|r| r.entries.as_slice())
}

/// Illusion of the invisible illusioner, kept at the client tick: `history` = previous
/// offsets (0..4) then current (4..8), `extra[0]` = transition ticks (3 → 0). At the last
/// tick of the damage overlay or every 1200 ticks: new offsets (±3 blocks per half block
/// in x and z, 0 or 1 in y); at the start of the overlay: copies brought back to the centre. True if the copies
/// have just been dispersed (clouds and sound to emit). `int(n)`: integer drawn in [0, n[.
pub(crate) fn illusion_tick(e: &mut Entity, mut int: impl FnMut(i32) -> i32) -> bool {
    if e.kind != "illusioner" || e.flags() & 0x20 == 0 {
        return false;
    }
    if e.history.len() != 8 {
        e.history = vec![[0.0; 3]; 8];
    }
    e.extra[0] = (e.extra[0] - 1.0).max(0.0);
    let scatter = e.hurt_time == 1 || e.age.is_multiple_of(1200);
    if scatter || e.hurt_time == 9 {
        e.extra[0] = 3.0;
        for i in 0..4 {
            e.history[i] = e.history[i + 4];
            e.history[i + 4] = if scatter {
                let x = f64::from(-6 + int(13)) * 0.5;
                let y = f64::from((int(6) - 4).max(0));
                let z = f64::from(-6 + int(13)) * 0.5;
                [x, y, z]
            } else {
                [0.0; 3]
            };
        }
    }
    scatter
}

/// Offsets of the four copies at fraction `pt`: current, or blend with the previous
/// ones during the transition (weight ((ticks − pt) / 3)^¼ for the previous ones).
fn illusion_offsets(e: &Entity, pt: f32) -> [[f64; 3]; 4] {
    if e.history.len() != 8 {
        return [[0.0; 3]; 4];
    }
    let cur = |i: usize| e.history[i + 4];
    if e.extra[0] <= 0.0 {
        return std::array::from_fn(cur);
    }
    let s = (f64::from((e.extra[0] - pt) / 3.0)).powf(0.25);
    std::array::from_fn(|i| std::array::from_fn(|k| cur(i)[k] * (1.0 - s) + e.history[i][k] * s))
}

/// Wobble of copy `i` at age `age` (ticks + fraction).
fn illusion_wobble(i: usize, age: f32) -> [f64; 3] {
    let i = i as f32;
    [
        f64::from((i + age * 0.5).cos()) * 0.025,
        f64::from((i + age * 0.75).cos()) * 0.0125,
        f64::from((i + age * 0.7).cos()) * 0.025,
    ]
}

/// Body layer of a model without its own rule: translucent for the allay (wing outline and
/// half-transparent halo), cutout otherwise.
fn default_body_pass(kind: &str) -> Pass {
    if kind == "allay" { Pass::Translucent } else { Pass::Cutout }
}

/// Avatar in spectator mode: body, arms and legs hidden with their skin layers; head and
/// hat remain.
fn spectator_head_only(p: &mut Pose<'_>) {
    for part in [
        "body",
        "right_arm",
        "left_arm",
        "right_leg",
        "left_leg",
        "jacket",
        "right_sleeve",
        "left_sleeve",
        "right_pants",
        "left_pants",
    ] {
        p.hide(part);
    }
}

/// Layer copied from the body that the game keeps as outline only when the entity, invisible, glows:
/// sheep wool (not the undercoat) and slime shell.
fn outlined_when_invisible(layer: &str) -> bool {
    matches!(layer, "minecraft:sheep#wool" | "minecraft:sheep_baby#wool" | "minecraft:slime#outer")
}

/// Layer drawn without damage overlay or white: animal equipment and wolf collar (the only
/// fixed-tint layer of the wolf).
fn no_damage_overlay(kind: &str, layer: &str, wool: Wool) -> bool {
    equipment_layer(layer) || (kind == "wolf" && matches!(wool, Wool::Fixed(_)))
}

/// Held item drawn: illagers depending on their pose (`entities-humanoids.md` § 4).
fn held_visible(e: &Entity) -> bool {
    let aggressive = e.meta_byte(15) & 4 != 0;
    let spell = e.meta_byte(17) > 0;
    match e.kind.as_str() {
        "vindicator" => aggressive,
        "evoker" => spell,
        "illusioner" => spell || aggressive,
        _ => true,
    }
}

/// Piglin ears (base angle 30°, 5° baby).
fn piglin_ears(p: &mut Pose<'_>, s: &AnimState) {
    let d = deg(if s.baby { 5.0 } else { 30.0 });
    let f = s.age * 0.1 + s.walk_pos * 0.5;
    let a = 0.08 + s.walk_speed * 0.4;
    p.set("left_ear", |e| e[5] = -d - (1.2 * f).cos() * a);
    p.set("right_ear", |e| e[5] = d + f.cos() * a);
}

/// Downward weapon strike (armed piglin, illagers): main arm M, other arm O.
fn strike_down(p: &mut Pose<'_>, s: &AnimState, right: bool) {
    let a2 = (s.swing * PI).sin();
    let a1 = ((1.0 - (1.0 - s.swing).powi(2)) * PI).sin();
    let (m, o) = if right { ("right_arm", "left_arm") } else { ("left_arm", "right_arm") };
    p.set("right_arm", |a| {
        a[5] = 0.0;
        a[4] = PI / 20.0;
    });
    p.set("left_arm", |a| {
        a[5] = 0.0;
        a[4] = -PI / 20.0;
    });
    p.set(m, |a| a[3] = -1.884_955_8 + (s.age * 0.09).cos() * 0.15 + a2 * 2.2 - a1 * 0.4);
    p.set(o, |a| a[3] = (s.age * 0.19).cos() * 0.5 + a2 * 1.2 - a1 * 0.4);
    idle_and_crouch(p, &AnimState { crouching: false, ..*s });
}

/// Arm pose of a piglin outside dancing, by priority.
fn piglin_pose_of(loved_offhand: bool, melee: bool, charging: bool, charged_crossbow: bool) -> u8 {
    if loved_offhand {
        piglin_pose::ADMIRING
    } else if melee {
        piglin_pose::MELEE
    } else if charging {
        piglin_pose::CHARGE
    } else if charged_crossbow {
        piglin_pose::HOLD
    } else {
        piglin_pose::DEFAULT
    }
}

/// Piglin and brute (§ 3.1): humanoid, ears, then dance, brandished weapon, crossbow (charge,
/// held) or admiration of an item.
fn piglin(p: &mut Pose<'_>, e: &Entity, s: &AnimState) {
    let right = e.meta_byte(15) & 2 == 0;
    let dancing = e.kind == "piglin" && e.meta_bool(19);
    let melee = !dancing && s.piglin == piglin_pose::MELEE;
    if melee && s.swing > 0.0 {
        // Humanoid without the generic attack, then the downward strike.
        humanoid(p, &AnimState { swing: 0.0, ..*s });
        strike_down(p, s, right);
    } else {
        humanoid(p, s);
    }
    piglin_ears(p, s);
    if dancing {
        let t = s.age / 60.0;
        p.set("right_ear", |x| x[5] = PI / 6.0 + deg((30.0 * t).sin() * 10.0));
        p.set("left_ear", |x| x[5] = -PI / 6.0 - deg((30.0 * t).cos() * 10.0));
        p.set("head", |h| {
            h[0] += (10.0 * t).sin();
            h[1] += (40.0 * t).sin() + 0.4;
        });
        let rz = deg(70.0 + (40.0 * t).cos() * 10.0);
        p.set("right_arm", |a| {
            a[5] = rz;
            a[1] += (40.0 * t).sin() * 0.5 - 0.5;
        });
        p.set("left_arm", |a| {
            a[5] = -rz;
            a[1] += (40.0 * t).sin() * 0.5 + 0.5;
        });
        p.set("body", |b| b[1] += (40.0 * t).sin() * 0.35);
    } else if melee {
        if s.swing == 0.0 {
            p.set(if right { "right_arm" } else { "left_arm" }, |a| a[3] = -1.8);
        }
    } else if s.piglin == piglin_pose::HOLD {
        crossbow_hold(p, s, right);
    } else if s.piglin == piglin_pose::CHARGE {
        crossbow_charge(p, s, right);
    } else if s.piglin == piglin_pose::ADMIRING {
        p.set("head", |h| {
            h[3] = 0.5;
            h[4] = 0.0;
        });
        // Arm opposite the main hand stretched towards the admired item.
        p.set(if right { "left_arm" } else { "right_arm" }, |a| {
            a[4] = if right { 0.5 } else { -0.5 };
            a[3] = -0.9;
        });
    }
}

/// Loaded crossbow held (`right`: held in the right arm): aimed by the head, the other
/// arm supporting; head angles of the pose (`s.pitch`, `s.yaw`).
fn crossbow_hold(p: &mut Pose<'_>, s: &AnimState, right: bool) {
    let sign = if right { 1.0 } else { -1.0 };
    let (me, other) = if right { ("right_arm", "left_arm") } else { ("left_arm", "right_arm") };
    p.set(me, |a| {
        a[4] = -sign * 0.3 + s.yaw;
        a[3] = -PI / 2.0 + s.pitch + 0.1;
    });
    p.set(other, |a| {
        a[4] = sign * 0.6 + s.yaw;
        a[3] = -1.5 + s.pitch;
    });
}

/// Crossbow charging (`right`: held in the right arm): the other arm draws the string over
/// the use ticks, up to the charge duration.
fn crossbow_charge(p: &mut Pose<'_>, s: &AnimState, right: bool) {
    let sign = if right { 1.0 } else { -1.0 };
    let (me, other) = if right { ("right_arm", "left_arm") } else { ("left_arm", "right_arm") };
    let k = s.use_ticks.clamp(0.0, s.crossbow_charge) / s.crossbow_charge;
    p.set(me, |a| {
        a[4] = -sign * 0.8;
        a[3] = -0.970_796_35;
    });
    p.set(other, |a| {
        a[4] = sign * (0.4 + k * (0.85 - 0.4));
        a[3] = -0.970_796_35 + k * (-PI / 2.0 + 0.970_796_35);
    });
}

/// Illager (§ 4): crossed arms, attack, spell, bow, celebration, neutral.
fn illager(p: &mut Pose<'_>, e: &Entity, s: &AnimState) {
    let (pp, v) = (s.walk_pos * 0.6662, s.walk_speed);
    p.set("head", |h| {
        h[3] = s.pitch;
        h[4] = s.yaw;
    });
    p.set("right_arm", |a| {
        a[3] = (pp + PI).cos() * v;
        a[4] = 0.0;
        a[5] = 0.0;
    });
    p.set("left_arm", |a| {
        a[3] = pp.cos() * v;
        a[4] = 0.0;
        a[5] = 0.0;
    });
    p.set("right_leg", |l| {
        l[3] = pp.cos() * 0.7 * v;
        l[4] = 0.0;
        l[5] = 0.0;
    });
    p.set("left_leg", |l| {
        l[3] = (pp + PI).cos() * 0.7 * v;
        l[4] = 0.0;
        l[5] = 0.0;
    });
    // Mounted: arms forward, legs spread in seated position (replaces the walk).
    if s.riding {
        for arm in ["right_arm", "left_arm"] {
            p.set(arm, |a| (a[3], a[4], a[5]) = (-PI / 5.0, 0.0, 0.0));
        }
        p.set("right_leg", |l| (l[3], l[4], l[5]) = (-1.413_716_7, PI / 10.0, 0.078_539_82));
        p.set("left_leg", |l| (l[3], l[4], l[5]) = (-1.413_716_7, -PI / 10.0, -0.078_539_82));
    }
    if e.kind != "illusioner" {
        p.hide("hat");
    }
    let aggressive = e.meta_byte(15) & 4 != 0;
    let spell = e.meta_byte(17) > 0;
    let celebrate = e.meta_bool(16);
    #[derive(PartialEq)]
    enum Arm {
        Crossed,
        Attack,
        Spell,
        Bow,
        Celebrate,
        Neutral,
        CrossbowHold,
        CrossbowCharge,
    }
    // Pillager: crossbow charging (index 17), otherwise crossbow held in one of the hands.
    let pose = match e.kind.as_str() {
        "pillager" if e.meta_bool(17) => Arm::CrossbowCharge,
        "pillager" if s.crossbow_held => Arm::CrossbowHold,
        "pillager" if aggressive => Arm::Attack,
        "pillager" => Arm::Neutral,
        "vindicator" if aggressive => Arm::Attack,
        "evoker" if spell => Arm::Spell,
        "illusioner" if spell => Arm::Spell,
        "illusioner" if aggressive => Arm::Bow,
        "vindicator" | "evoker" if celebrate => Arm::Celebrate,
        _ => Arm::Crossed,
    };
    if pose == Arm::Crossed {
        p.hide("right_arm");
        p.hide("left_arm");
    } else {
        p.hide("arms");
    }
    match pose {
        Arm::Attack if e.equipment[0].is_none() => zombie_arms(p, s, true, true),
        Arm::Attack => strike_down(p, s, e.meta_byte(15) & 2 == 0),
        // The pillager always holds the crossbow in the right arm, whatever its main hand.
        Arm::CrossbowHold => crossbow_hold(p, s, true),
        Arm::CrossbowCharge => crossbow_charge(p, s, true),
        Arm::Spell => {
            let x = (s.age * 0.6662).cos() * 0.25;
            p.set("right_arm", |a| {
                a[0] = -5.0;
                a[2] = 0.0;
                a[3] = x;
                a[4] = 0.0;
                a[5] = 3.0 * PI / 4.0;
            });
            p.set("left_arm", |a| {
                a[0] = 5.0;
                a[2] = 0.0;
                a[3] = x;
                a[4] = 0.0;
                a[5] = -3.0 * PI / 4.0;
            });
        }
        Arm::Bow => {
            p.set("right_arm", |a| {
                a[4] = -0.1 + s.yaw;
                a[3] = -PI / 2.0 + s.pitch;
            });
            p.set("left_arm", |a| {
                a[3] = -0.942_477_9 + s.pitch;
                a[4] = s.yaw - 0.4;
                a[5] = PI / 2.0;
            });
        }
        Arm::Celebrate => {
            let x = (s.age * 0.6662).cos() * 0.05;
            p.set("right_arm", |a| {
                a[0] = -5.0;
                a[2] = 0.0;
                a[3] = x;
                a[4] = 0.0;
                a[5] = 2.670_354;
            });
            p.set("left_arm", |a| {
                a[0] = 5.0;
                a[2] = 0.0;
                a[3] = x;
                a[4] = 0.0;
                a[5] = -3.0 * PI / 4.0;
            });
        }
        _ => {}
    }
}

/// Vex (§ 4.6): arms, charge, wings.
fn vex(p: &mut Pose<'_>, e: &Entity, s: &AnimState) {
    p.set("head", |h| {
        h[3] = s.pitch;
        h[4] = s.yaw;
    });
    let b = deg(s.age * 5.5).cos() * 0.1;
    p.set("right_arm", |a| a[5] = PI / 5.0 + b);
    p.set("left_arm", |a| a[5] = -(PI / 5.0 + b));
    if e.meta_byte(16) & 1 != 0 {
        p.set("body", |x| x[3] = 0.0);
        let (r, l) = (e.equipment[0].is_some(), e.equipment[1].is_some());
        if !r && !l {
            p.set("right_arm", |a| {
                *a = [a[0], a[1], a[2], -1.221_730_5, PI / 12.0, -0.471_238_88 - b, a[6], a[7], a[8]]
            });
            p.set("left_arm", |a| {
                *a = [a[0], a[1], a[2], -1.221_730_5, -PI / 12.0, 0.471_238_88 + b, a[6], a[7], a[8]]
            });
        } else {
            if r {
                p.set("right_arm", |a| {
                    *a = [a[0], a[1], a[2], 7.0 * PI / 6.0, PI / 12.0, -0.471_238_88 - b, a[6], a[7], a[8]]
                });
            }
            if l {
                p.set("left_arm", |a| {
                    *a = [a[0], a[1], a[2], 7.0 * PI / 6.0, -PI / 12.0, 0.471_238_88 + b, a[6], a[7], a[8]]
                });
            }
        }
    } else {
        p.set("body", |x| x[3] = PI / 20.0);
    }
    let w = 1.099_557_4 + deg(s.age * 45.836_624).cos() * deg(16.2);
    p.set("left_wing", |x| {
        x[4] = w;
        x[3] = 0.471_238_88;
        x[5] = -0.471_238_88;
    });
    p.set("right_wing", |x| {
        x[4] = -w;
        x[3] = 0.471_238_88;
        x[5] = 0.471_238_88;
    });
}

/// Ravager (§ 4.8): attack, stun, roar, legs.
fn ravager(p: &mut Pose<'_>, e: &Entity, s: &AnimState, pt: f32) {
    let a = if e.timers[0] > 0 { e.timers[0] as f32 - pt } else { 0.0 };
    let st = if e.timers[1] > 0 { e.timers[1] as f32 - pt } else { 0.0 };
    let g = if e.timers[2] > 0 { (20.0 - e.timers[2] as f32 + pt) / 20.0 } else { 0.0 };
    if a > 0.0 {
        let wave = |x: f32, per: f32| ((x.rem_euclid(per) - per / 2.0).abs() - per / 4.0) / (per / 4.0);
        let h = ((1.0 + wave(a, 10.0)) * 0.5).powi(3) * 12.0;
        p.set("neck", |n| {
            n[2] = -6.5 + h;
            n[1] = -7.0;
        });
        let m = if a > 5.0 { ((a - 4.0) / 4.0).sin() * 0.4 * PI } else { PI / 20.0 * (PI * a / 10.0).sin() };
        p.set("mouth", |x| x[3] = m);
    } else {
        p.set("neck", |n| {
            n[0] = 0.0;
            n[1] = -7.0;
            n[2] = 5.5;
        });
        if st > 0.0 {
            p.set("neck", |n| {
                n[3] = 0.219_911_49;
                n[0] = (st / 40.0 * 10.0).sin() * 3.0;
            });
            p.set("mouth", |x| x[3] = 0.05 * PI);
        } else {
            let m = if g > 0.0 { PI / 2.0 * (g * PI * 0.25).sin() } else { 0.01 * PI };
            p.set("mouth", |x| x[3] = m);
        }
    }
    p.set("head", |h| {
        h[3] = s.pitch;
        h[4] = s.yaw;
    });
    let q = s.walk_pos * 0.6662;
    let l = 0.4 * s.walk_speed;
    for leg in ["right_hind_leg", "left_front_leg"] {
        p.set(leg, |x| x[3] = q.cos() * l);
    }
    for leg in ["left_hind_leg", "right_front_leg"] {
        p.set(leg, |x| x[3] = (q + PI).cos() * l);
    }
}

/// Hoglin and zoglin (§ 5): ears, head butt, legs.
fn hoglin(p: &mut Pose<'_>, e: &Entity, s: &AnimState) {
    let (pp, v) = (s.walk_pos, s.walk_speed);
    p.set("right_ear", |x| x[5] = -2.0 * PI / 9.0 - v * pp.sin());
    p.set("left_ear", |x| x[5] = 2.0 * PI / 9.0 + v * pp.sin());
    p.set("head", |h| h[4] = s.yaw);
    let r = e.timers[0] as f32;
    let f = 1.0 - (10.0 - 2.0 * r).abs() / 10.0;
    p.set("head", |h| {
        h[3] = 0.872_664_63 + (-PI / 9.0 - 0.872_664_63) * f;
        if s.baby {
            h[1] += f * 2.5;
        }
    });
    let (a, b) = (pp.cos() * 1.2 * v, (pp + PI).cos() * 1.2 * v);
    p.set("right_front_leg", |x| x[3] = a);
    p.set("left_front_leg", |x| x[3] = b);
    p.set("right_hind_leg", |x| x[3] = b);
    p.set("left_hind_leg", |x| x[3] = a);
}

/// Armor stand (§ 6): received poses, rods, base, flags.
fn armor_stand(p: &mut Pose<'_>, e: &Entity) {
    let rot = |i: u8, d: [f32; 3]| match e.meta.get(&i) {
        Some(rc_proto_v777::entity::Meta::Rotations(r)) => {
            r.map(|v| if v.is_finite() { deg(v.rem_euclid(360.0)) } else { 0.0 })
        }
        _ => d.map(deg),
    };
    let set = |p: &mut Pose<'_>, name: &str, r: [f32; 3]| {
        p.set(name, |x| {
            x[3] = r[0];
            x[4] = r[1];
            x[5] = r[2];
        })
    };
    let body = rot(17, [0.0; 3]);
    set(p, "head", rot(16, [0.0; 3]));
    set(p, "body", body);
    set(p, "left_arm", rot(18, [-10.0, 0.0, -10.0]));
    set(p, "right_arm", rot(19, [-15.0, 0.0, 10.0]));
    set(p, "left_leg", rot(20, [-1.0, 0.0, -1.0]));
    set(p, "right_leg", rot(21, [1.0, 0.0, 1.0]));
    for stick in ["right_body_stick", "left_body_stick", "shoulder_stick"] {
        set(p, stick, body);
    }
    p.set("base_plate", |x| x[4] = -deg(e.yaw));
    p.hide("hat");
    let flags = e.meta_byte(15);
    if flags & 0x04 == 0 {
        p.hide("right_arm");
        p.hide("left_arm");
    }
    if flags & 0x08 != 0 {
        p.hide("base_plate");
    }
}

/// Evoker fangs (§ 4.7): jaws and rising from the ground depending on progress.
fn fangs(p: &mut Pose<'_>, e: &Entity, pt: f32) {
    let b = if !e.started {
        0.0
    } else {
        let r = e.timers[0] - 2;
        if r <= 0 { 1.0 } else { 1.0 - (r as f32 - pt) / 20.0 }
    };
    if b == 0.0 {
        for part in ["base", "upper_jaw", "lower_jaw"] {
            p.hide(part);
        }
        return;
    }
    let m = 1.0 - (2.0 * b).min(1.0).powi(3);
    p.set("upper_jaw", |x| x[5] = PI - 0.35 * PI * m);
    p.set("lower_jaw", |x| x[5] = PI + 0.35 * PI * m);
    p.set("base", |x| x[1] -= (b + (2.7 * b).sin()) * 7.2);
    let k = if b > 0.9 { (1.0 - b) / 0.1 } else { 1.0 };
    p.set("root", |x| {
        x[1] = 24.0 - 20.0 * k;
        x[6] = k;
        x[7] = k;
        x[8] = k;
    });
}

/// Visual rotation (degrees) of a ground item, seen by the needles: half-turn minus its
/// own rotation at the middle of tick `age` (without the frame fraction).
fn ground_item_yaw(age: u32, bob_offs: f32) -> f32 {
    180.0 - ((age as f32 + 0.5) / 20.0 + bob_offs) / (PI * 2.0) * 360.0
}

/// Evaluation without a world (frames, while waiting for their carrier): given context, carrier present.
struct HeldEnv(&'static str);

impl rc_assets::item::EvalEnv for HeldEnv {
    fn display_context(&self) -> &str {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Original formulas (chained rotation matrices, inverse transpose).
    fn reference(p: &[f32; 9]) -> (Mat4, glam::Mat3) {
        let t = Mat4::from_translation(Vec3::new(p[0] / 16.0, p[1] / 16.0, p[2] / 16.0));
        let r = Mat4::from_rotation_z(p[5]) * Mat4::from_rotation_y(p[4]) * Mat4::from_rotation_x(p[3]);
        let m = t * r * Mat4::from_scale(Vec3::new(p[6], p[7], p[8]));
        (m, glam::Mat3::from_mat4(m).inverse().transpose())
    }

    /// Fixed lights of 3D items: unit (rotation and mirror), distinct from those of the
    /// world and independent of the dimension.
    #[test]
    fn items_3d_lights_are_fixed_unit_vectors() {
        let l = items_3d_lights();
        for v in l {
            let n = Vec3::new(v[0], v[1], v[2]).length();
            assert!((n - 1.0).abs() < 1e-5, "{n}");
        }
        assert_ne!(l, world_lights(false));
        assert_ne!(l, world_lights(true));
    }

    /// Cube faces turning counter-clockwise seen from the outside (normal), mirror included:
    /// condition to cull back faces without losing a front face.
    #[test]
    fn cube_faces_wind_outward() {
        for mirror in [false, true] {
            let c = rc_data::EntityCube {
                uv: [0.0, 0.0],
                from: [-2.0, -3.0, -1.0],
                size: [4.0, 6.0, 2.0],
                grow: [0.25; 3],
                mirror,
                tex_scale: [1.0, 1.0],
                faces: 0x3f,
            };
            let mut quads = Vec::new();
            bake_cube(&c, [64, 32], &mut quads);
            assert_eq!(quads.len(), 6);
            for q in &quads {
                let [a, b, d] = [q.pos[0], q.pos[1], q.pos[2]].map(Vec3::from);
                let n = (b - a).cross(d - a).normalize();
                let center = q.pos.iter().map(|&p| Vec3::from(p)).sum::<Vec3>() / 4.0;
                // Outgoing geometric normal (cube centre at (0, 0, 0)).
                assert!(n.dot(center) > 0.0, "mirror {mirror}: face {:?} turned inwards", q.normal);
                assert!(n.dot(Vec3::from(q.normal)) > 0.99, "mirror {mirror}: normal {:?} ≠ {n}", q.normal);
            }
        }
    }

    use super::keyframes::testkit;
    use rc_proto_v777::entity::Meta;

    /// Valid entity shader (emissive, flat, scrolling flags included).
    #[test]
    fn entity_shader_validates() {
        let m = wgpu::naga::front::wgsl::parse_str(include_str!("entity.wgsl")).expect("parse");
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&m)
        .expect("validation");
    }

    /// "jeb_" wool: white (0xE6E6E6) at tick 0, orange × 0.75 at tick 25, halfway at tick
    /// 12.5 (truncated integer fade).
    #[test]
    fn jeb_wool_cycles_dye_colors() {
        assert_eq!(jeb_wool(0.0), 0xffe6_e6e6);
        assert_eq!(jeb_wool(25.0), wool_color(1));
        let o = wool_color(1);
        let mid = jeb_wool(12.5);
        let r = |c: u32| (c >> 16 & 0xff) as i32;
        assert_eq!(r(mid), 0xe6 + ((r(o) - 0xe6) as f32 * 0.5).floor() as i32);
        assert_eq!(jeb_wool(400.0), wool_color(0), "16 × 25 tick cycle");
    }

    /// Type hat: hidden under a full-hat profession, or partial against a type with a
    /// full hat (snow farmer: without snow hat).
    #[test]
    fn mannequin_skin_from_profile() {
        // Empty partial profile: null id, skin 0 for all mannequins.
        assert_eq!(profile_uuid(&[0, 0, 0, 0, 0]), Some(0));
        let mut all = testkit::spawn("mannequin");
        let e = all.map.get_mut(&7).unwrap();
        e.uuid = 0x1234_5678_9abc_def0_1122_3344_5566_7788;
        assert_eq!(avatar_default_skin(e), default_skin(0), "without profile: not the entity's id");
        // Named partial: offline id of the name.
        let mut b = vec![0, 1, 10];
        b.extend_from_slice(b"Dinnerbone");
        b.extend_from_slice(&[0, 0, 0]);
        assert_eq!(profile_uuid(&b), Some(rc_client::offline_uuid("Dinnerbone")));
        // Full: its id.
        let mut b = vec![1];
        b.extend_from_slice(&7u128.to_be_bytes());
        b.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        assert_eq!(profile_uuid(&b), Some(7));
        e.meta.insert(17, Meta::Profile(b.into()));
        assert_eq!(avatar_default_skin(e), default_skin(7));
    }

    /// Static profile: `textures` property and skin patch read, without an online lookup.
    #[test]
    fn mannequin_profile_reads_textures_and_patch() {
        let s = |b: &mut Vec<u8>, t: &str| {
            b.push(t.len() as u8);
            b.extend_from_slice(t.as_bytes());
        };
        // Full: id, name, one signed property, then patch (body + slim model).
        let mut b = vec![1];
        b.extend_from_slice(&5u128.to_be_bytes());
        s(&mut b, "Bob");
        b.push(1);
        s(&mut b, "textures");
        s(&mut b, "VALUE");
        b.push(1);
        s(&mut b, "sig");
        b.push(1);
        s(&mut b, "minecraft:entity/player/wide/zuri");
        b.extend_from_slice(&[0, 0, 1, 1]);
        let p = parse_profile(&b).unwrap();
        assert_eq!(p.uuid, 5);
        assert_eq!(p.textures.as_deref(), Some("VALUE"));
        assert_eq!(p.body.as_deref(), Some("minecraft:entity/player/wide/zuri"));
        assert_eq!((p.cape, p.elytra, p.slim), (None, None, Some(true)));
        // Partial with properties: static, offline name and property kept.
        let mut b = vec![0, 1];
        s(&mut b, "Bob");
        b.push(0);
        b.push(1);
        s(&mut b, "textures");
        s(&mut b, "V2");
        b.extend_from_slice(&[0, 0, 0, 0, 0]);
        let p = parse_profile(&b).unwrap();
        assert_eq!((p.uuid, p.textures.as_deref(), p.slim), (rc_client::offline_uuid("Bob"), Some("V2"), None));
    }

    #[test]
    fn villager_type_hat_rule() {
        assert!(type_hat_visible(2, 0));
        assert!(!type_hat_visible(0, 2));
        assert!(!type_hat_visible(2, 1));
        assert!(type_hat_visible(0, 1));
    }

    /// Alpha of the warden layers: spots in opposite phase, heart 1 → 0.9 at the beat.
    #[test]
    fn warden_pulse_alphas() {
        let mut all = testkit::spawn("warden");
        let e = all.map.get_mut(&7).unwrap();
        e.age = 0;
        assert!((pulse_alpha(e, 1, 0.0) - 0.25).abs() < 1e-6 && pulse_alpha(e, 2, 0.0) == 0.0);
        assert!((pulse_alpha(e, 4, 0.0) - 1.0).abs() < 1e-6 && (pulse_alpha(e, 4, 1.0) - 0.9).abs() < 1e-6);
        e.age = 39;
        assert_eq!(pulse_alpha(e, 4, 0.5), 0.0);
        e.meta.insert(16, Meta::Int(80));
        e.age = 20;
        assert!((pulse_alpha(e, 4, 0.0) - 1.0).abs() < 1e-6, "maximum anger: delay 10");
    }

    fn rot_ctx() -> RotCtx {
        RotCtx {
            shaking: false,
            upside_down: false,
            fall_flying: false,
            swim: 0.0,
            env: PoseEnv::default(),
            height: 1.8,
        }
    }

    /// Swim: +0.09 per tick in "swim" pose up to 1, then −0.09; before, the counter
    /// did not exist (swimming player drawn standing).
    #[test]
    fn swim_amount_ramps_like_the_game() {
        let mut all = testkit::spawn("player");
        all.map.get_mut(&7).unwrap().meta.insert(6, Meta::Pose(3));
        for _ in 0..5 {
            all.tick();
        }
        let a = all.map[&7].pose_anim;
        assert!((a.swim - 0.45).abs() < 1e-5 && (a.swim(0.5) - 0.405).abs() < 1e-5, "{a:?}");
        for _ in 0..20 {
            all.tick();
        }
        assert_eq!(all.map[&7].pose_anim.swim, 1.0);
        all.map.get_mut(&7).unwrap().meta.insert(6, Meta::Pose(0));
        all.tick();
        assert!((all.map[&7].pose_anim.swim - 0.91).abs() < 1e-5);
    }

    /// Wings: 30% of the way per tick towards the target (crouching: 2π/9, 5°, −π/4) from 0.
    #[test]
    fn elytra_angles_ease_towards_target() {
        let mut all = testkit::spawn("player");
        all.map.get_mut(&7).unwrap().meta.insert(6, Meta::Pose(5));
        all.tick();
        let r = all.map[&7].pose_anim.elytra(1.0);
        assert!((r[0] - 0.3 * PI * 2.0 / 9.0).abs() < 1e-6, "{r:?}");
        assert!((r[1] - 0.3 * 0.087_266_46).abs() < 1e-6 && (r[2] + 0.3 * PI / 4.0).abs() < 1e-6, "{r:?}");
        for _ in 0..60 {
            all.tick();
        }
        let r = all.map[&7].pose_anim.elytra(1.0);
        assert!((r[0] - PI * 2.0 / 9.0).abs() < 1e-4 && (r[2] + PI / 4.0).abs() < 1e-4, "{r:?}");
    }

    /// Glide: tick counter, scale t² / 100 capped; use: ticks since the start.
    #[test]
    fn fall_fly_and_use_counters() {
        let mut all = testkit::spawn("player");
        {
            let e = all.map.get_mut(&7).unwrap();
            e.meta.insert(0, Meta::Byte(0x80u8 as i8));
            e.meta.insert(8, Meta::Byte(1));
        }
        for _ in 0..4 {
            all.tick();
        }
        let a = all.map[&7].pose_anim;
        assert_eq!((a.fly_ticks, a.use_ticks), (4, 4));
        assert!((a.fly_scale(0.0) - 0.16).abs() < 1e-6 && a.fly_scale(10.0 - 4.0) == 1.0);
        all.map.get_mut(&7).unwrap().meta.insert(0, Meta::Byte(0));
        all.tick();
        assert_eq!(all.map[&7].pose_anim.fly_ticks, 0);
    }

    /// Flight yaw: movement towards +x, gaze towards +z (yaw 0) → +π/2.
    #[test]
    fn flying_yaw_follows_motion() {
        let mut all = testkit::spawn("player");
        let e = all.map.get_mut(&7).unwrap();
        e.velocity = [1.0, 0.0, 0.0];
        e.pose_anim.vel_o = [1.0, 0.0, 0.0];
        let a = flying_y_rot(e, 0.0).unwrap();
        assert!((a - PI / 2.0).abs() < 1e-5, "{a}");
        e.velocity = [0.0; 3];
        e.pose_anim.vel_o = [0.0; 3];
        assert!(flying_y_rot(e, 0.0).is_none());
    }

    /// Breaststroke at time 7 of the cycle: right arm (x, y, z) = (0, π, π − 1.8708 × q(7)/q(14)),
    /// legs 0.3 cos(7/3); head at −π/4.
    #[test]
    fn swim_stroke_matches_formula() {
        let parts = [("head", None), ("right_arm", None), ("left_arm", None), ("right_leg", None), ("left_leg", None)];
        let l = testkit::test_layer(&parts);
        let all = testkit::spawn("player");
        let mut s = testkit::state(&all.map[&7], 0.0);
        s.walk_pos = 7.0;
        s.swim = 1.0;
        let mut p = Pose::reset(&l, Vec::new(), Vec::new());
        humanoid(&mut p, &s);
        let q = |x: f32| -65.0 * x + x * x;
        let ra = p.parts[1];
        assert!(ra[3].abs() < 1e-5 && (ra[4] - PI).abs() < 1e-5, "{ra:?}");
        assert!((ra[5] - (PI - 1.870_796_4 * q(7.0) / q(14.0))).abs() < 1e-5, "{ra:?}");
        assert!((p.parts[3][3] - 0.3 * (7.0f32 / 3.0).cos()).abs() < 1e-5);
        assert!((p.parts[0][3] + PI / 4.0).abs() < 1e-5);
    }

    fn close(a: Mat4, b: Mat4) {
        assert!(a.abs_diff_eq(b, 1e-4), "{a:?}\n≠\n{b:?}");
    }

    /// Bed: without body yaw, Y(direction) Z(90) Y(270); Riptide: X(−90 − pitch) then
    /// spinning top; upside down: raised by height + 0.1 and flipped; shaking: offset yaw.
    #[test]
    fn special_pose_rotations() {
        let mut all = testkit::spawn("player");
        let e = all.map.get_mut(&7).unwrap();
        e.meta.insert(6, Meta::Pose(2));
        let mut c = rot_ctx();
        c.env.bed = Some((1.0, 0.0, 180.0));
        let mut m = Mat4::IDENTITY;
        pose_rotations(e, 0.0, 30.0, &mut m, &c);
        close(m, Mat4::from_rotation_y(PI) * Mat4::from_rotation_z(PI / 2.0) * Mat4::from_rotation_y(deg(270.0)));

        e.meta.insert(6, Meta::Pose(0));
        e.meta.insert(8, Meta::Byte(4));
        e.age = 2;
        e.pitch = 10.0;
        e.pitch_o = 10.0;
        let mut m = Mat4::IDENTITY;
        pose_rotations(e, 0.0, 30.0, &mut m, &rot_ctx());
        close(
            m,
            Mat4::from_rotation_y(deg(150.0)) * Mat4::from_rotation_x(deg(-100.0)) * Mat4::from_rotation_y(deg(-150.0)),
        );

        e.meta.insert(8, Meta::Byte(0));
        let mut c = rot_ctx();
        c.upside_down = true;
        let mut m = Mat4::IDENTITY;
        pose_rotations(e, 0.0, 30.0, &mut m, &c);
        close(
            m,
            Mat4::from_rotation_y(deg(150.0))
                * Mat4::from_translation(Vec3::new(0.0, 1.9, 0.0))
                * Mat4::from_rotation_z(PI),
        );

        let mut c = rot_ctx();
        c.shaking = true;
        let mut m = Mat4::IDENTITY;
        pose_rotations(e, 0.0, 30.0, &mut m, &c);
        let shake = (2.0f32 * 3.25).cos() * PI * 0.4;
        close(m, Mat4::from_rotation_y(deg(180.0 - 30.0 - shake)));
    }

    /// Avatar swimming in water: X(w × (−90 − pitch)) then (0, −1, 0.3) in "swim" pose.
    #[test]
    fn swimming_avatar_lies_down() {
        let mut all = testkit::spawn("player");
        let e = all.map.get_mut(&7).unwrap();
        e.meta.insert(6, Meta::Pose(3));
        e.pitch = 20.0;
        e.pitch_o = 20.0;
        let mut c = rot_ctx();
        c.swim = 0.5;
        c.env.in_water = true;
        let mut m = Mat4::IDENTITY;
        pose_rotations(e, 0.0, 0.0, &mut m, &c);
        close(
            m,
            Mat4::from_rotation_y(PI)
                * Mat4::from_rotation_x(deg(0.5 * -110.0))
                * Mat4::from_translation(Vec3::new(0.0, -1.0, 0.3)),
        );
    }

    /// Saddle, armors and carpet remain drawn on an invisible animal; markings and coat do not.
    #[test]
    fn equipment_layers_survive_invisibility() {
        for n in [
            "minecraft:horse#saddle",
            "minecraft:camel#saddle",
            "minecraft:nautilus#saddle",
            "minecraft:llama#decor",
            "minecraft:llama_baby#decor",
            "minecraft:horse_armor#main",
            "minecraft:undead_horse_armor#main",
            "minecraft:nautilus_armor#main",
            "minecraft:happy_ghast_harness#main",
        ] {
            assert!(equipment_layer(n), "{n}");
        }
        for n in ["minecraft:horse#main", "minecraft:sheep#wool", "minecraft:wither#armor", "minecraft:wolf#collar"] {
            assert!(!equipment_layer(n), "{n}");
        }
    }

    /// Glint scroll: whole milliseconds × speed × 8, periods 110 s and 30 s.
    #[test]
    fn glint_scroll_matches_game_formula() {
        let (u, v) = glint_offsets(1000.0, 0.5);
        assert_eq!((u, v), (4000.0 / 110_000.0, 4000.0 / 30_000.0));
        // 31 s at speed 0.5: 124000 ms → (14000 / 110000, 4000 / 30000).
        let (u, v) = glint_offsets(31_000.0, 0.5);
        assert_eq!((u, v), (14_000.0 / 110_000.0, 4000.0 / 30_000.0));
        // Truncation: 0.1 ms × 8 → 0.
        assert_eq!(glint_offsets(0.1, 1.0), (0.0, 0.0));
    }

    /// Passes: distinct indices, all drawn once, opaques before the translucents.
    #[test]
    fn pass_order_covers_every_pass() {
        let mut seen = [false; PASSES];
        for p in PASS_ORDER {
            assert!(!seen[p], "pass {p} duplicated");
            seen[p] = true;
        }
        assert!(seen.iter().all(|&s| s));
        let pos = |p: usize| PASS_ORDER.iter().position(|&q| q == p).unwrap_or(usize::MAX);
        for opaque in [0, PASS_CUTOUT_CULL, PASS_SOLID, 5] {
            for later in [1, PASS_TRANSLUCENT_CULL, 2, 6] {
                assert!(pos(opaque) < pos(later));
            }
        }
        for p in [Pass::Cutout, Pass::Translucent, Pass::Eyes, Pass::Swirl, Pass::CutoutCull, Pass::Solid] {
            assert!(p.index() < PASSES);
        }
        // Classic translucent phase: items and moving blocks, then entity models (including the
        // ghost of an invisible entity, back faces culled).
        assert!(pos(PASS_TRANSLUCENT_SOLID) < pos(PASS_ITEM_TRANSLUCENT));
        assert!(pos(PASS_ITEM_TRANSLUCENT) < pos(1));
        assert!(pos(1) < pos(PASS_TRANSLUCENT_CULL));
        assert!(pos(PASS_TRANSLUCENT_CULL) < pos(2));
    }

    /// Special models: only the heads are blended (pass 1); chest, banner, shield... are
    /// drawn before the translucent items (pass 12), in classic as in improved transparency.
    #[test]
    fn special_models_drawn_before_translucent_items() {
        let pos = |p: usize| PASS_ORDER.iter().position(|&q| q == p).unwrap_or(usize::MAX);
        for kind in 3..=10 {
            for oit in [false, true] {
                assert!(pos(special_pass(oit, kind)) < pos(PASS_ITEM_TRANSLUCENT), "kind {kind}");
            }
        }
        for kind in 1..=2 {
            assert_eq!(special_pass(false, kind), 1);
            assert_eq!(special_pass(true, kind), PASS_TRANSLUCENT_SOLID);
        }
    }

    /// Translucent items and blocks: from farthest to nearest, distance to the pose origin
    /// (item) or to the cube centre (block), placement order kept on ties.
    #[test]
    fn translucent_items_sort_far_to_near() {
        let at = |x: f32| PartMat::new(&Mat4::from_translation(Vec3::new(x, 0.0, 0.0)));
        let mats = [at(1.0), at(5.0), at(-3.0), at(0.5)];
        let inst = |base: u32| Instance { base, rect: [0.0; 4], color: 0, light: 0, overlay: 0 };
        let mut list = vec![
            (1, 10, 1, inst(0)),
            (1, 11, 1, inst(1)),
            (1, 12, 1, inst(2)),
            (1, 13, 1, inst(0)),
            // Block placed at x = 0.5: centre (1, 0.5, 0.5), distance² 1.5, farther than the items
            // at x = 1 (distance² 1); taken at its corner, it would be the nearest (0.25).
            (1 | SRC_BLOCK_CENTER, 14, 1, inst(3)),
        ];
        sort_far_to_near(&mut list, &mats, Vec3::ZERO);
        let order: Vec<u32> = list.iter().map(|i| i.1).collect();
        assert_eq!(order, [11, 12, 14, 10, 13]);
    }

    #[test]
    fn fast_sin_cos_is_precise() {
        let mut worst = 0.0f32;
        let mut x = -20.0f32;
        while x < 20.0 {
            let (s, c) = sin_cos(x);
            worst = worst.max((s - x.sin()).abs()).max((c - x.cos()).abs());
            x += 0.000_37;
        }
        assert!(worst < 1e-6, "{worst}");
    }

    #[test]
    fn fast_part_matrix_matches_reference() {
        let mut seed = 0x1234_5678u32;
        let mut rnd = |a: f32, b: f32| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            a + (b - a) * (seed as f32 / u32::MAX as f32)
        };
        for k in 0..2000 {
            let rot = if k % 4 == 0 { [0.0; 3] } else { [rnd(-3.2, 3.2), rnd(-3.2, 3.2), rnd(-3.2, 3.2)] };
            let sx = if k % 7 == 0 { -1.0 } else { rnd(0.5, 1.5) };
            let p = [
                rnd(-20.0, 20.0),
                rnd(-20.0, 20.0),
                rnd(-20.0, 20.0),
                rot[0],
                rot[1],
                rot[2],
                sx,
                rnd(0.5, 1.5),
                rnd(0.5, 1.5),
            ];
            let (m_ref, n_ref) = reference(&p);
            let m = part_matrix(&p);
            assert!(m.abs_diff_eq(m_ref, 1e-5), "{p:?}");
            let pm = PartMat::new(&m);
            let n = glam::Mat3::from_cols(
                Vec3::from_slice(&pm.n[0][..3]),
                Vec3::from_slice(&pm.n[1][..3]),
                Vec3::from_slice(&pm.n[2][..3]),
            );
            // Same normal directions (up to a positive factor).
            for v in [Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(0.3, -0.5, 0.8)] {
                let (a, b) = ((n * v).normalize(), (n_ref * v).normalize());
                assert!(a.abs_diff_eq(b, 1e-4), "{p:?} {v:?}");
            }
        }
    }
}
