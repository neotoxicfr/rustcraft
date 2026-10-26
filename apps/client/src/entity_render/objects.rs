//! Non-living entities and projectiles (`docs/specs/entities-objects.md`): sprites of thrown
//! items, frames, boats, minecarts, TNT, falling blocks, projectiles with a model, End
//! crystal, display entities, fishing bobber.
//!
//! Common "block in an entity" capability (§ 13.1): a block state (baked terrain
//! models, textures `#block/<index>` of the atlas) or a named block model outside states
//! (frames) is baked once into the object mesh buffer, then drawn like an
//! item with the wanted matrix.

use rc_assets::geom::Dir;
use rc_proto_v777::entity::Meta;
use rc_proto_v777::item::ItemStack;

use super::*;

/// Quad of a named block model: positions as a fraction of a block, UV as a fraction of the texture
/// `tex` (atlas name), normal, tint index.
pub(crate) struct ModelQuad {
    pos: [[f32; 3]; 4],
    uv: [[f32; 2]; 4],
    tex: String,
    normal: [f32; 3],
}

/// Named block models drawn in entities (dummy frame states, § 3.2).
const NAMED_MODELS: [&str; 4] = [
    "minecraft:block/item_frame",
    "minecraft:block/item_frame_map",
    "minecraft:block/glow_item_frame",
    "minecraft:block/glow_item_frame_map",
];

/// Corners of a face, official order of the block models (0 = min, 1 = max on x, y, z).
const CORNERS: [[[u8; 3]; 4]; 6] = [
    [[0, 0, 1], [0, 0, 0], [1, 0, 0], [1, 0, 1]],
    [[0, 1, 0], [0, 1, 1], [1, 1, 1], [1, 1, 0]],
    [[1, 1, 0], [1, 0, 0], [0, 0, 0], [0, 1, 0]],
    [[0, 1, 1], [0, 0, 1], [1, 0, 1], [1, 1, 1]],
    [[0, 1, 0], [0, 0, 0], [0, 0, 1], [0, 1, 1]],
    [[1, 1, 1], [1, 0, 1], [1, 0, 0], [1, 1, 0]],
];

/// Default UV of a face (pixels 0..16).
fn default_uv(f: [f32; 3], t: [f32; 3], d: Dir) -> [f32; 4] {
    match d {
        Dir::Down => [f[0], 16.0 - t[2], t[0], 16.0 - f[2]],
        Dir::Up => [f[0], f[2], t[0], t[2]],
        Dir::North => [16.0 - t[0], 16.0 - t[1], 16.0 - f[0], 16.0 - f[1]],
        Dir::South => [f[0], 16.0 - t[1], t[0], 16.0 - f[1]],
        Dir::West => [f[2], 16.0 - t[1], t[2], 16.0 - f[1]],
        Dir::East => [16.0 - t[2], 16.0 - t[1], 16.0 - f[2], 16.0 - f[1]],
    }
}

/// Bakes the named models (elements, element rotation) and adds their textures to the atlas
/// (`#tex/<id>`).
pub(super) fn load_named_models(
    assets: &rc_assets::source::Assets,
    sources: &mut HashMap<String, crate::entity_atlas::Source>,
) -> HashMap<String, Vec<ModelQuad>> {
    let models = rc_assets::model::Models::new(assets);
    let mut out = HashMap::new();
    for id in NAMED_MODELS {
        let Some(m) = models.flatten(id) else { continue };
        let mut quads = Vec::new();
        for e in m.elements.iter() {
            for d in Dir::ALL {
                let Some(face) = &e.faces[d as usize] else { continue };
                let Some(t) = m.resolve(&face.texture) else { continue };
                let (ns, path) = rc_assets::source::split_id(&t.value);
                let tex = format!("#tex/{ns}:{path}");
                if !sources.contains_key(&tex)
                    && let Some(img) = rc_assets::texture::load(assets, &format!("{ns}:{path}"))
                {
                    sources.insert(tex.clone(), crate::entity_atlas::Source::Image(img));
                }
                let uvs = face.uv.unwrap_or_else(|| default_uv(e.from, e.to, d));
                let mut pos = [[0.0f32; 3]; 4];
                let mut uv = [[0.0f32; 2]; 4];
                for i in 0..4 {
                    let c = CORNERS[d as usize][i];
                    let mut p: [f32; 3] = [0, 1, 2].map(|a| if c[a] == 1 { e.to[a] } else { e.from[a] } / 16.0);
                    if let Some(r) = &e.rotation {
                        let q = rc_assets::geom::mul_v(
                            &r.matrix,
                            [p[0] - r.origin[0], p[1] - r.origin[1], p[2] - r.origin[2]],
                        );
                        p = [q[0] + r.origin[0], q[1] + r.origin[1], q[2] + r.origin[2]];
                    }
                    pos[i] = p;
                    let j = (i + face.rotation as usize) % 4;
                    let u = if j == 0 || j == 1 { uvs[0] } else { uvs[2] } / 16.0;
                    let v = if j == 0 || j == 3 { uvs[1] } else { uvs[3] } / 16.0;
                    uv[i] = [u, v];
                }
                let a = Vec3::from(pos[1]) - Vec3::from(pos[0]);
                let b = Vec3::from(pos[2]) - Vec3::from(pos[1]);
                let n = a.cross(b).normalize_or_zero();
                quads.push(ModelQuad { pos, uv, tex, normal: n.to_array() });
            }
        }
        out.insert(id.to_owned(), quads);
    }
    out
}

/// State specific to non-living entities.
pub(super) struct Objects {
    blocks: Option<Arc<rc_assets::BlockModels>>,
    named: HashMap<String, Vec<ModelQuad>>,
    /// Baked meshes (in `item_verts`): key → index into `meshes`.
    block_meshes: HashMap<u32, usize>,
    named_meshes: HashMap<&'static str, usize>,
    /// Groups (tint, first vertex, count) of each mesh.
    meshes: Vec<Vec<(i32, u32, u32)>>,
    /// Translucent mesh (at least one quad of the translucent layer), parallel to `meshes`.
    mesh_translucent: Vec<bool>,
    /// Item ids by name (default stacks of projectiles).
    item_ids: HashMap<&'static str, i32>,
    /// Default block states by name (minecarts), lit furnace separate.
    block_states: HashMap<&'static str, u32>,
    /// Default colour of the grass palette (ARGB).
    pub(super) grass_default: u32,
    /// Block entity whose special model is added to the state's model (kind, variant, on a wall,
    /// state properties), per state; `None`: state model alone.
    specials: HashMap<u32, Option<(crate::block_entities::Kind, u8, bool, crate::block_entities::Props)>>,
    /// Faces of the End portals drawn as entities (camera-relative frame), taken over
    /// by `push_block_entities` after its clearing.
    pub(super) portals: Vec<crate::block_fx::PortalVertex>,
    /// Layer of the boats' water mask.
    water_patch: Option<Option<usize>>,
    /// Lightning bolts: fixed slots of the mesh buffer (seed, first vertex), the oldest
    /// rewritten first.
    bolts: Vec<(u64, u32)>,
    bolt_next: usize,
    /// Text displays of the frame (drawn by `text_display.rs`).
    pub(super) texts: Vec<TextDraw>,
    /// Names of the map decorations of the frame (designated by `TextDraw::label`).
    pub(super) labels: Vec<rc_proto::nbt::Nbt>,
    /// Bobbers whose owner player is known (only these are drawn), and segments of their
    /// lines (frame relative to the camera origin) of the frame.
    bobber_owners: hashbrown::HashSet<i32>,
    /// Players with an active bobber (rod "cast" in the hand that holds the line).
    pub(crate) fishers: hashbrown::HashSet<i32>,
    /// Id of the frame's local player (only carrier whose selected slot and current use
    /// count for held items).
    pub(crate) local: i32,
    lines: Vec<([f32; 3], [f32; 3])>,
    /// Guardians' eye of the frame (entity → x, y offset of the "eye" part), turned towards
    /// the attack target, otherwise towards the camera entity.
    pub(super) guardian_eyes: hashbrown::HashMap<i32, [f32; 2]>,
}

/// Colours of a block drawn as an entity: position-less colours of its tint sources
/// (carried, displayed, in a minecart, TNT block), or colours of the block's biome (grass,
/// foliage, dry foliage, water; RGB) for a moving block, falling or transient.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BlockTint {
    Entity,
    World([u32; 4]),
}

/// Text display placed this frame: entity, matrix (font pixels → camera frame, half-turn and
/// scale −0.025 included), light, interpolated state.
#[derive(Clone, Copy, Debug)]
pub struct TextDraw {
    pub id: i32,
    pub m: Mat4,
    pub light: u32,
    /// Text opacity (byte, 255 = opaque) and ARGB background colour (0 = none).
    pub opacity: u8,
    pub background: u32,
    pub flags: i8,
    /// Map decoration name: index into `text_labels` (text placed as is at frame
    /// `m` before centring), `u32::MAX` for a text display (text of entity `id`).
    pub label: u32,
    /// Drawn with the hand camera (held map).
    pub hand: bool,
}

/// Lightning slots kept in the mesh buffer.
const BOLT_SLOTS: usize = 16;
/// Vertices of a lightning bolt: 4 layers × 14 segments × 4 quads × 6.
const BOLT_VERTS: u32 = 4 * 14 * 4 * 6;

/// Geometry of a lightning bolt (§ 11) in blocks: quads (4 corners) in drawing order.
fn bolt_quads(seed: u64) -> Vec<[[f32; 3]; 4]> {
    use rc_assets::bake::Lcg;
    let mut r = Lcg::new(seed as i64);
    let (mut xs, mut zs) = ([0.0f32; 8], [0.0f32; 8]);
    let (mut xo, mut zo) = (0.0f32, 0.0f32);
    for k in (0..8).rev() {
        xs[k] = xo;
        zs[k] = zo;
        xo += r.next_int(11) as f32 - 5.0;
        zo += r.next_int(11) as f32 - 5.0;
    }
    const SIGNS: [(f32, f32, f32, f32); 4] =
        [(-1.0, -1.0, 1.0, -1.0), (1.0, -1.0, 1.0, 1.0), (1.0, 1.0, -1.0, 1.0), (-1.0, 1.0, -1.0, -1.0)];
    let mut out = Vec::with_capacity((BOLT_VERTS / 6) as usize);
    for c in 0..4 {
        let mut r = Lcg::new(seed as i64);
        for b in 0..3usize {
            let trunk = b == 0;
            let k0 = 7 - b;
            let k1 = if trunk { 0 } else { k0 - 2 };
            let (mut sx, mut sz) = (xs[k0] - xo, zs[k0] - zo);
            for k in (k1..=k0).rev() {
                let (ex, ez) = (sx, sz);
                let span = if trunk { 11 } else { 31 };
                let half = (span / 2) as f32;
                sx += r.next_int(span) as f32 - half;
                sz += r.next_int(span) as f32 - half;
                let base = 0.1 + 0.2 * c as f32;
                let (mut top, mut bottom) = (base, base);
                if trunk {
                    top *= 0.1 * k as f32 + 1.0;
                    bottom *= 0.1 * (k as f32 - 1.0) + 1.0;
                }
                let (y0, y1) = (16.0 * k as f32, 16.0 * (k + 1) as f32);
                for (rx, rz, lx, lz) in SIGNS {
                    out.push([
                        [sx + rx * bottom, y0, sz + rz * bottom],
                        [ex + rx * top, y1, ez + rz * top],
                        [ex + lx * top, y1, ez + lz * top],
                        [sx + lx * bottom, y0, sz + lz * bottom],
                    ]);
                }
            }
        }
    }
    out
}

impl Objects {
    pub(super) fn new(
        blocks: Option<Arc<rc_assets::BlockModels>>,
        named: HashMap<String, Vec<ModelQuad>>,
        grass_default: u32,
    ) -> Self {
        Objects {
            blocks,
            named,
            block_meshes: HashMap::new(),
            named_meshes: HashMap::new(),
            meshes: Vec::new(),
            mesh_translucent: Vec::new(),
            item_ids: HashMap::new(),
            block_states: HashMap::new(),
            grass_default,
            specials: HashMap::new(),
            portals: Vec::new(),
            water_patch: None,
            bolts: Vec::new(),
            bolt_next: 0,
            texts: Vec::new(),
            labels: Vec::new(),
            bobber_owners: Default::default(),
            fishers: Default::default(),
            local: -1,
            guardian_eyes: Default::default(),
            lines: Vec::new(),
        }
    }
}

/// Wood of a boat or raft: (layer, texture).
fn boat_layer(k: &str) -> Option<(String, String)> {
    let (wood, chest) = if let Some(w) = k.strip_suffix("_chest_boat") {
        (w, true)
    } else if let Some(w) = k.strip_suffix("_boat") {
        (w, false)
    } else if let Some(w) = k.strip_suffix("_chest_raft") {
        (w, true)
    } else {
        (k.strip_suffix("_raft")?, false)
    };
    let dir = if chest { "chest_boat" } else { "boat" };
    Some((format!("minecraft:{dir}/{wood}#main"), format!("entity/{dir}/{wood}")))
}

fn is_boat(k: &str) -> bool {
    k.ends_with("_boat") || k.ends_with("_raft")
}

/// Kinds drawn by `push_object` (all its branches except the last): the others
/// leave without doing anything.
pub(super) fn object_kind(k: &str) -> bool {
    matches!(
        k,
        "snowball"
            | "egg"
            | "ender_pearl"
            | "eye_of_ender"
            | "splash_potion"
            | "lingering_potion"
            | "experience_bottle"
            | "fireball"
            | "small_fireball"
            | "firework_rocket"
            | "dragon_fireball"
            | "cushion"
            | "fishing_bobber"
            | "lightning_bolt"
            | "item_frame"
            | "glow_item_frame"
            | "tnt"
            | "falling_block"
            | "leash_knot"
            | "end_crystal"
            | "wither_skull"
            | "shulker_bullet"
            | "llama_spit"
            | "wind_charge"
            | "breeze_wind_charge"
            | "trident"
            | "block_display"
            | "item_display"
            | "text_display"
            | "ominous_item_spawner"
    ) || is_boat(k)
        || is_minecart(k)
}

fn is_minecart(k: &str) -> bool {
    k == "minecart" || k.ends_with("_minecart")
}

pub(super) fn layers(
    _r: &EntityRenderer,
    k: &str,
    _baby: bool,
    skin: u32,
) -> Option<Vec<(String, String, Pass, Wool)>> {
    let one = |m: &str, t: &str, p: Pass| Some(vec![(m.to_owned(), t.to_owned(), p, Wool::None)]);
    match k {
        _ if is_boat(k) => {
            let (m, t) = boat_layer(k)?;
            Some(vec![(m, t, Pass::Cutout, Wool::None)])
        }
        _ if is_minecart(k) => one(&format!("minecraft:{k}#main"), "entity/minecart/minecart", Pass::Cutout),
        "leash_knot" => one("minecraft:leash_knot#main", "entity/lead_knot/lead_knot", Pass::Cutout),
        "end_crystal" => one("minecraft:end_crystal#main", "entity/end_crystal/end_crystal", Pass::Cutout),
        // `skin` 1: "dangerous" skull.
        "wither_skull" => one(
            "minecraft:wither_skull#main",
            if skin == 1 { "entity/wither/wither_invulnerable" } else { "entity/wither/wither" },
            Pass::Translucent,
        ),
        "shulker_bullet" => one("minecraft:shulker_bullet#main", "entity/shulker/spark", Pass::Cutout),
        "llama_spit" => one("minecraft:llama_spit#main", "entity/llama/llama_spit", Pass::Cutout),
        "wind_charge" | "breeze_wind_charge" => {
            one("minecraft:wind_charge#main", "entity/projectiles/wind_charge", Pass::Translucent)
        }
        "trident" => one("minecraft:trident#main", "entity/trident/trident", Pass::Solid),
        // `skin`: dye colour (meta 8).
        "cushion" => one(
            "minecraft:cushion#main",
            &format!("entity/cushion/{}_cushion", crate::block_entities::DYES[(skin & 15) as usize]),
            Pass::Cutout,
        ),
        _ => None,
    }
}

pub(super) fn shadow(k: &str) -> Option<(f32, f32)> {
    match k {
        _ if is_boat(k) => Some((0.8, 1.0)),
        _ if is_minecart(k) => Some((0.7, 1.0)),
        "tnt" | "falling_block" | "end_crystal" => Some((0.5, 1.0)),
        "item_frame"
        | "glow_item_frame"
        | "leash_knot"
        | "snowball"
        | "egg"
        | "ender_pearl"
        | "eye_of_ender"
        | "splash_potion"
        | "lingering_potion"
        | "experience_bottle"
        | "fireball"
        | "small_fireball"
        | "dragon_fireball"
        | "firework_rocket"
        | "wither_skull"
        | "shulker_bullet"
        | "llama_spit"
        | "wind_charge"
        | "breeze_wind_charge"
        | "trident"
        | "fishing_bobber"
        | "lightning_bolt"
        | "block_display"
        | "item_display"
        | "text_display"
        | "interaction"
        | "marker"
        | "area_effect_cloud"
        | "ominous_item_spawner"
        // No shadow radius of its own (0 by default for non-living entities).
        | "arrow"
        | "spectral_arrow"
        | "painting"
        | "cushion" => Some((0.0, 0.0)),
        _ => None,
    }
}

/// Client tick: paddles (positions in `extra[0..2]`) and bubble column tilt
/// (multiplier `extra[4]`, current and previous angle `extra[5..7]`).
pub(super) fn tick(e: &mut Entity) {
    if !is_boat(&e.kind) {
        return;
    }
    for s in 0..2 {
        if e.meta_bool(11 + s as u8) {
            e.extra[s] += PI / 8.0;
        } else {
            e.extra[s] = 0.0;
        }
    }
    let bubbles = e.meta_int(13).is_some_and(|t| t > 0);
    let mult = (e.extra[4] + if bubbles { 0.05 } else { -0.1 }).clamp(0.0, 1.0);
    e.extra[4] = mult;
    e.extra[6] = e.extra[5];
    e.extra[5] = 10.0 * (0.5 * e.age as f32).sin() * mult;
}

/// Packed light (block × 16 | sky × 16 << 8).
/// Network context of an item display → index into `DISPLAY_CONTEXTS` (`none`: centring only).
pub(super) fn display_context(context: i8) -> Option<usize> {
    match context {
        1 => Some(3),
        2 => Some(2),
        3 => Some(5),
        4 => Some(4),
        5 => Some(6),
        6 => Some(0),
        7 => Some(1),
        8 => Some(7),
        9 => Some(8),
        _ => None,
    }
}

fn pack(b: u8, s: u8) -> u32 {
    (u32::from(b) * 16) | ((u32::from(s) * 16) << 8)
}

/// Neutral and white overlay (§ 4.1: u = 15, row 10).
const NEUTRAL: u32 = 10 << 16;
const WHITE: u32 = 15 | 10 << 16;

fn meta_float(e: &Entity, i: u8) -> Option<f32> {
    match e.meta.get(&i) {
        Some(Meta::Float(f)) => Some(*f),
        _ => None,
    }
}

fn meta_vec3(e: &Entity, i: u8) -> Option<[f32; 3]> {
    match e.meta.get(&i) {
        Some(Meta::Vector3(v)) => Some(*v),
        _ => None,
    }
}

/// Raw quaternion [x, y, z, w]: the network sends it as is, without normalisation.
fn meta_quat(e: &Entity, i: u8) -> Option<[f32; 4]> {
    match e.meta.get(&i) {
        Some(Meta::Quaternion(q)) => Some(*q),
        _ => None,
    }
}

fn meta_state(e: &Entity, i: u8) -> Option<u32> {
    match e.meta.get(&i) {
        Some(Meta::BlockState(s)) => Some(s.and_then(|s| u32::try_from(s).ok()).unwrap_or(0)),
        _ => None,
    }
}

/// Offset of the received position towards (block centre in x and z, bottom of block + `dy`).
fn block_offset(p: [f64; 3], dy: f64) -> Vec3 {
    let f = |v: f64, o: f64| (v.floor() + o - v) as f32;
    Vec3::new(f(p[0], 0.5), f(p[1], dy), f(p[2], 0.5))
}

/// Visual rotation of a frame (degrees, in [−180, 180[): half-turn, plus a quarter turn
/// per step of the horizontal direction (south 0, west 1, north 2, east 3; −1 on the floor and the
/// ceiling), plus 45° per notch of the item's rotation, plus ±90° hung on the ceiling or the floor.
pub(super) fn frame_visual_yaw(dir: i32, rotation: i32) -> f32 {
    let (d2, corr) = match dir {
        0 => (-1, -90),
        1 => (-1, 90),
        2 => (2, 0),
        4 => (1, 0),
        5 => (3, 0),
        _ => (0, 0),
    };
    crate::debug_screen::wrap_degrees((180 + d2 * 90 + rotation * 45 + corr) as f32)
}

/// Pose of the ominous item spawner at age `age` (ticks + fraction): scale age/50 while
/// the age does not exceed 50, then yaw of 40° per tick.
pub(super) fn ominous_spawner_pose(age: f32) -> Mat4 {
    let scale = if age <= 50.0 { Mat4::from_scale(Vec3::splat(age.min(50.0) / 50.0)) } else { Mat4::IDENTITY };
    scale * Mat4::from_rotation_y(deg(crate::debug_screen::wrap_degrees(age * 40.0)))
}

/// Light and overlay words of the wind charge: flags "no cardinal lighting" and
/// "scrolling", offset u = age × 0.03 (fraction, in 1/65535) in the overlay word.
fn wind_charge_scroll(light: u32, age: f32) -> (u32, u32) {
    let u = (age * 0.03 % 1.0).rem_euclid(1.0);
    (light | super::LIGHT_FLAT | super::LIGHT_SCROLL, (u * 65535.0) as u32)
}

/// Yaw (shortest-path interpolation) and pitch (linear) of the Shulker bullet,
/// in degrees.
fn shulker_bullet_angles(e: &Entity, pt: f32) -> (f32, f32) {
    (rot_lerp(pt, e.yaw_o, e.yaw), e.pitch_o + (e.pitch - e.pitch_o) * pt)
}

/// Anti-flicker offset of a minecart (±0.002 block per axis), drawn from its id
/// like the game: 64-bit seed, 3 bits per axis at ranks 16, 20 and 24.
pub(super) fn minecart_offset(id: i32) -> Vec3 {
    let s = i64::from(id).wrapping_mul(493_286_711);
    let seed = s.wrapping_mul(s).wrapping_mul(4_392_167_121).wrapping_add(s.wrapping_mul(98_761));
    let axis = |shift: u32| (((seed >> shift) & 7) as f32 + 0.5) / 8.0 - 0.5;
    Vec3::new(axis(16), axis(20), axis(24)) * 0.004
}

/// Hit shake of vehicles (§ 1.3 step 3).
fn hurt_shake(e: &Entity, pt: f32) -> Mat4 {
    let h = e.meta_int(8).unwrap_or(0) as f32 - pt;
    if h <= 0.0 {
        return Mat4::IDENTITY;
    }
    let d = (meta_float(e, 10).unwrap_or(0.0) - pt).max(0.0);
    let dir = e.meta_int(9).unwrap_or(1) as f32;
    Mat4::from_rotation_x(deg(h.sin() * h * d / 10.0 * dir))
}

/// Swelling and blinking of TNT (§ 4.1) for f = fuse − pt + 1.
fn tnt_swell(f: f32) -> (f32, bool) {
    let g = if f < 10.0 { (1.0 - f / 10.0).clamp(0.0, 1.0).powi(4) * 0.3 } else { 0.0 };
    (g, f >= 0.0 && (f / 5.0).floor() as i32 % 2 == 0)
}

/// Own time of an End crystal (§ 7.1): counter (start drawn from the UUID, the game draws it
/// at random) + age + pt.
pub(super) fn crystal_time(e: &Entity, pt: f32) -> f32 {
    ((e.uuid as u32) % 100_000) as f32 + e.age as f32 + pt
}

/// Height of the floating crystal (§ 7.2), in blocks.
pub(super) fn crystal_height(a: f32) -> f32 {
    let h = (0.2 * a).sin() / 2.0 + 0.5;
    (h * h + h) * 0.4 - 1.4
}

/// Block targeted by the crystal's beam (meta 8).
pub(super) fn beam_target(e: &Entity) -> Option<[i32; 3]> {
    match e.meta.get(&8) {
        Some(Meta::BlockPos(Some(b))) => Some(*b),
        _ => None,
    }
}

/// Crystal beam (`entities-animals.md` § 15.1, `entities-objects.md` § 7.3) in frame
/// `m`, towards vector `d`: 8 quads (ring 0.15 black at the start, 0.75 white at the end), v
/// scrolling with `time`.
pub(super) fn beam_quads(m: Mat4, d: Vec3, time: f32, rect: [f32; 4], light: u32, out: &mut Vec<FreeVertex>) {
    let horizontal = (d.x * d.x + d.z * d.z).sqrt();
    let len = d.length();
    let m = m
        * Mat4::from_translation(Vec3::new(0.0, 2.0, 0.0))
        * Mat4::from_rotation_y(-d.z.atan2(d.x) - PI / 2.0)
        * Mat4::from_rotation_x(-horizontal.atan2(d.y) - PI / 2.0);
    let normal = m.transform_vector3(Vec3::new(0.0, -1.0, 0.0)).normalize_or_zero().to_array();
    let (v0, v1) = (-time * 0.01, len / 32.0 - time * 0.01);
    let vert = |x: f32, y: f32, z: f32, u: f32, v: f32, color: u32| FreeVertex {
        pos: m.transform_point3(Vec3::new(x, y, z)).to_array(),
        uv: [u, v],
        rect,
        color,
        light,
        normal,
    };
    const BLACK: u32 = 0xff00_0000;
    const WHITE: u32 = 0xffff_ffff;
    let (mut ls, mut lc, mut lu) = (0.0f32, 0.75f32, 0.0f32);
    for i in 1..=8 {
        let a = i as f32 * (PI * 2.0) / 8.0;
        let (s, c) = (crate::sky_mesh::table_sin(a) * 0.75, crate::sky_mesh::table_cos(a) * 0.75);
        let u = i as f32 / 8.0;
        let q = [
            vert(ls * 0.2, lc * 0.2, 0.0, lu, v0, BLACK),
            vert(ls, lc, len, lu, v1, WHITE),
            vert(s, c, len, u, v1, WHITE),
            vert(s * 0.2, c * 0.2, 0.0, u, v0, BLACK),
        ];
        out.extend([0, 1, 2, 2, 3, 0].map(|k| q[k]));
        (ls, lc, lu) = (s, c, u);
    }
}

impl EntityRenderer {
    /// Crystal beam in the beam list (texture loaded on first use).
    pub(super) fn push_beam(&mut self, m: Mat4, d: Vec3, time: f32, light: u32) {
        let Some(rect) = self.atlas.rect("entity/end_crystal/end_crystal_beam") else { return };
        beam_quads(m, d, time, rect, light, &mut self.free_verts[FREE_BEAM]);
    }

    /// Draws a non-living entity of this family: `None` if the kind is not part of it,
    /// otherwise true if something was drawn.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_object(
        &mut self,
        e: &Entity,
        pt: f32,
        rel: [f32; 3],
        p: [f64; 3],
        cam: [f64; 3],
        light: &dyn Fn(i32, i32, i32) -> (u8, u8),
        items: Option<&rc_assets::item::ItemModels>,
        world: &super::ShadowWorld<'_>,
    ) -> Option<bool> {
        let k = e.kind.as_str();
        // Kind outside the family: nothing to probe or compute.
        if self.opt.obj_early
            && !(if self.opt.kind_info { self.kind_of(e).has(super::K_OBJECT) } else { object_kind(k) })
        {
            return None;
        }
        // Light probe: block of the eyes (interpolated position + eye height of the type).
        let (b, s) = self.eye_light(e, p, light);
        let b = if e.flags() & 1 != 0 { 15 } else { b };
        let normal = pack(b, s);
        let bright = pack(15, s);
        let t = Mat4::from_translation(Vec3::from(rel));
        let age = e.age as f32 + pt;
        Some(match k {
            "snowball" | "egg" | "ender_pearl" | "eye_of_ender" | "splash_potion" | "lingering_potion"
            | "experience_bottle" | "fireball" | "small_fireball" => {
                // Hidden at the start: not stuck to the shooter's screen.
                let d2: f64 = (0..3).map(|i| (p[i] - cam[i]).powi(2)).sum();
                if e.age < 2 && d2 < 12.25 {
                    return Some(false);
                }
                let (item, scale, lit) = match k {
                    "eye_of_ender" => ("minecraft:ender_eye", 1.0, true),
                    "fireball" => ("minecraft:fire_charge", 3.0, true),
                    "small_fireball" => ("minecraft:fire_charge", 0.75, true),
                    "snowball" => ("minecraft:snowball", 1.0, false),
                    "egg" => ("minecraft:egg", 1.0, false),
                    "ender_pearl" => ("minecraft:ender_pearl", 1.0, false),
                    "splash_potion" => ("minecraft:splash_potion", 1.0, false),
                    "lingering_potion" => ("minecraft:lingering_potion", 1.0, false),
                    _ => ("minecraft:experience_bottle", 1.0, false),
                };
                let m = t * Mat4::from_scale(Vec3::splat(scale)) * self.cam_rot;
                let Some(models) = items else { return Some(false) };
                self.push_meta_stack(e, 8, item, models, 1, m, if lit { bright } else { normal })
            }
            "firework_rocket" => {
                let mut m = t * self.cam_rot;
                if e.meta_bool(10) {
                    m *= Mat4::from_rotation_z(PI) * Mat4::from_rotation_y(PI) * Mat4::from_rotation_x(PI / 2.0);
                }
                let Some(models) = items else { return Some(false) };
                self.push_meta_stack(e, 8, "minecraft:firework_rocket", models, 1, m, normal)
            }
            "ominous_item_spawner" => {
                let Some(models) = items else { return Some(false) };
                let Some(Meta::Item(Some(stack))) = e.meta.get(&8) else { return Some(false) };
                if stack.count <= 0 {
                    return Some(false);
                }
                // Grows over 50 ticks, spins 40° per tick, full light; copies depending on the
                // quantity like a ground item (no bobbing or lift).
                let pre = t * ominous_spawner_pose(age);
                self.push_item_cluster(models, stack, (e.id, e.pos, e.yaw), pre, None, Mat4::IDENTITY, 0xf0f0)
            }
            "dragon_fireball" => {
                let Some(rect) = self.atlas.rect("entity/enderdragon/dragon_fireball") else { return Some(false) };
                let m = t * Mat4::from_scale(Vec3::splat(2.0)) * self.cam_rot;
                self.push_quad(m, rect, 0, bright);
                true
            }
            "cushion" => {
                // Orientation: horizontal direction of the yaw (nearest quarter turn, in
                // double like the game).
                let dir = ((f64::from(e.yaw) / 90.0 + 0.5).floor() as i32 & 3) as f32 * 90.0;
                let m = t
                    * Mat4::from_rotation_y(deg(180.0 - dir))
                    * Mat4::from_rotation_x(PI)
                    * Mat4::from_translation(Vec3::new(0.0, -0.25, 0.0));
                self.push_layers(e, m, normal, |_| {})
            }
            "fishing_bobber" => {
                // Nothing without a known owner player (the game draws neither bobber nor line).
                if !self.objects.bobber_owners.contains(&e.id) {
                    return Some(false);
                }
                let Some(rect) = self.atlas.rect("entity/fishing/fishing_hook") else { return Some(false) };
                let m = t
                    * Mat4::from_scale(Vec3::splat(0.5))
                    * self.cam_rot
                    * Mat4::from_translation(Vec3::new(0.0, -0.25, 0.0));
                // Bobber: cutout with culled back faces (quad turned towards the camera).
                self.push_quad(m, rect, super::PASS_CUTOUT_CULL, normal);
                true
            }
            // Hanging entities: received position = hanging block; the client recomputes the
            // position (frame: block centre moved back by 0.46875 towards the wall, which the stack cancels;
            // knot: block centre + 0.375).
            "lightning_bolt" => self.push_bolt(e, t),
            "item_frame" | "glow_item_frame" => {
                let c = Mat4::from_translation(block_offset(p, 0.5));
                self.push_frame(e, t * c, light, p, items)
            }
            _ if is_boat(k) => self.push_boat(e, pt, t, normal),
            _ if is_minecart(k) => self.push_minecart(e, pt, t, p, normal, world.state),
            "tnt" => {
                let f = e.meta_int(8).unwrap_or(80) as f32 - pt + 1.0;
                let (g, on) = tnt_swell(f);
                let state = meta_state(e, 9).unwrap_or_else(|| self.default_state("minecraft:tnt"));
                let m = t
                    * Mat4::from_translation(Vec3::new(0.0, 0.5, 0.0))
                    * Mat4::from_scale(Vec3::splat(1.0 + g))
                    * Mat4::from_rotation_y(deg(-90.0))
                    * Mat4::from_translation(Vec3::new(-0.5, -0.5, 0.5))
                    * Mat4::from_rotation_y(deg(90.0));
                self.push_block(state, m, normal, if on { WHITE } else { NEUTRAL })
            }
            "falling_block" => {
                let state = u32::try_from(e.data).unwrap_or(0);
                // Same state already placed in the world (landing): no duplicate.
                if (world.state)(p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32) == state {
                    return Some(false);
                }
                let m = t * Mat4::from_translation(Vec3::new(-0.5, 0.0, -0.5));
                // Tint: biome of the containing block (x, top of the box, z).
                let top = p[1] + f64::from(e.dims(&self.dims)[1]);
                let biome = (world.biome_tints)(p[0].floor() as i32, top.floor() as i32, p[2].floor() as i32);
                self.push_block_tinted(state, m, normal, NEUTRAL, BlockTint::World(biome))
            }
            "leash_knot" => {
                let m =
                    t * Mat4::from_translation(block_offset(p, 0.375)) * Mat4::from_scale(Vec3::new(-1.0, -1.0, 1.0));
                self.push_layers(e, m, normal, |_| {})
            }
            "end_crystal" => {
                let a = crystal_time(e, pt);
                let height = crystal_height(a);
                // Beam towards the centre of the targeted block (§ 7.3), outside the model scale.
                if let Some(b) = beam_target(e) {
                    let d = Vec3::new(
                        (f64::from(b[0]) + 0.5 - p[0]) as f32,
                        (f64::from(b[1]) + 0.5 - p[1]) as f32,
                        (f64::from(b[2]) + 0.5 - p[2]) as f32,
                    );
                    self.push_beam(t * Mat4::from_translation(d), Vec3::new(-d.x, -d.y + height, -d.z), a, normal);
                }
                let base = !matches!(e.meta.get(&9), Some(Meta::Bool(false)));
                let m = t * Mat4::from_scale(Vec3::splat(2.0)) * Mat4::from_translation(Vec3::new(0.0, -0.5, 0.0));
                let spin = glam::Quat::from_rotation_y(deg(3.0 * a));
                let tilt = glam::Quat::from_axis_angle(Vec3::new(1.0, 0.0, 1.0).normalize(), PI / 3.0);
                let euler = |q: glam::Quat| q.to_euler(glam::EulerRot::ZYX);
                let (oz, oy, ox) = euler(spin * tilt);
                let (iz, iy, ix) = euler(tilt * spin);
                self.push_layers(e, m, normal, |p| {
                    if !base {
                        p.hide("base");
                    }
                    p.set("outer_glass", |x| {
                        x[1] += height * 16.0 / 2.0;
                        (x[3], x[4], x[5]) = (ox, oy, oz);
                    });
                    for part in ["inner_glass", "cube"] {
                        p.set(part, |x| (x[3], x[4], x[5]) = (ix, iy, iz));
                    }
                })
            }
            "wither_skull" => {
                let yaw = rot_lerp(pt, e.yaw_o, e.yaw);
                let pitch = e.pitch_o + (e.pitch - e.pitch_o) * pt;
                let m = t * Mat4::from_scale(Vec3::new(-1.0, -1.0, 1.0));
                self.push_layers(e, m, bright, |p| {
                    p.set("head", |x| {
                        x[4] = deg(yaw);
                        x[3] = deg(pitch);
                    })
                })
            }
            "shulker_bullet" => {
                let m = t
                    * Mat4::from_translation(Vec3::new(0.0, 0.15, 0.0))
                    * Mat4::from_rotation_y(deg((0.1 * age).sin() * 180.0))
                    * Mat4::from_rotation_x(deg((0.1 * age).cos() * 180.0))
                    * Mat4::from_rotation_z(deg((0.15 * age).sin() * 360.0))
                    * Mat4::from_scale(Vec3::new(-0.5, -0.5, 0.5));
                // Single part oriented by the entity's interpolated yaw and pitch.
                let (yaw, pitch) = shulker_bullet_angles(e, pt);
                let orient = |p: &mut Pose<'_>| {
                    p.set("main", |x| {
                        x[4] = deg(yaw);
                        x[3] = deg(pitch);
                    })
                };
                let drawn = self.push_layers(e, m, bright, orient);
                // Translucent halo (alpha 38/255) at 1.5 times the size.
                if drawn {
                    let li = self.resolve(e.kind_id, k, false, 0);
                    if let Some(&r) = self.arena[li].first() {
                        let halo = m * Mat4::from_scale(Vec3::splat(1.5));
                        self.push_resolved(r, halo, bright, NEUTRAL, 0x26ff_ffff, Some(1), orient);
                    }
                }
                drawn
            }
            "llama_spit" => {
                let yaw = rot_lerp(pt, e.yaw_o, e.yaw);
                let pitch = e.pitch_o + (e.pitch - e.pitch_o) * pt;
                let m = t
                    * Mat4::from_translation(Vec3::new(0.0, 0.15, 0.0))
                    * Mat4::from_rotation_y(deg(yaw - 90.0))
                    * Mat4::from_rotation_z(deg(pitch));
                self.push_layers(e, m, normal, |_| {})
            }
            "wind_charge" | "breeze_wind_charge" => {
                // Render type of the breeze's wind: translucent without cardinal lighting or
                // veil, texture scrolling in u by 0.03 per tick.
                let li = self.resolve(e.kind_id, k, false, 0);
                let (light, scroll) = wind_charge_scroll(normal, age);
                for i in 0..self.arena[li].len() {
                    let r = self.arena[li][i];
                    self.push_resolved(r, t, light, scroll, 0xffff_ffff, Some(1), |p| {
                        p.set("wind_charge", |x| x[4] = deg(-16.0 * age));
                        p.set("wind", |x| x[4] = deg(16.0 * age));
                    });
                }
                !self.arena[li].is_empty()
            }
            "trident" => {
                let yaw = e.yaw_o + (e.yaw - e.yaw_o) * pt;
                let pitch = e.pitch_o + (e.pitch - e.pitch_o) * pt;
                let m = t * Mat4::from_rotation_y(deg(yaw - 90.0)) * Mat4::from_rotation_z(deg(pitch + 90.0));
                // Meta 12: enchanted trident (entity glint in the shader).
                let glint = if e.meta_bool(12) { super::GLINT_ENTITY } else { 0 };
                self.push_layers(e, m, normal | glint, |_| {})
            }
            "block_display" | "item_display" => {
                // Nothing until the first tick has frozen the render state (§ 13.5).
                let Some(ds) = e.display.as_deref() else { return Some(false) };
                let Some(rs) = &ds.render else { return Some(false) };
                let progress = ds.progress(e.age, pt);
                ds.last.set(progress);
                let m = self.display_matrix(e, pt, t, rs.billboard, &rs.pose(progress));
                // Forced brightness: block and sky levels frozen from meta 16.
                let lw = match rs.brightness {
                    v if v != -1 => pack(((v >> 4) & 15) as u8, ((v >> 20) & 15) as u8),
                    _ => normal,
                };
                match &rs.sub {
                    DisplaySub::Block(state) => *state != 0 && self.push_block(*state, m, lw, NEUTRAL),
                    DisplaySub::Item(stack, ctx) => {
                        let Some(models) = items else { return Some(false) };
                        let Some(stack) = stack else { return Some(false) };
                        self.push_display_item(e, models, stack, *ctx, m, lw)
                    }
                    DisplaySub::Text(_) => false,
                }
            }
            "text_display" => {
                let Some(ds) = e.display.as_deref() else { return Some(false) };
                let Some(rs) = &ds.render else { return Some(false) };
                let DisplaySub::Text(text) = &rs.sub else { return Some(false) };
                let progress = ds.progress(e.age, pt);
                ds.last.set(progress);
                let lw = match rs.brightness {
                    v if v != -1 => pack(((v >> 4) & 15) as u8, ((v >> 20) & 15) as u8),
                    _ => normal,
                };
                // Text frame: half-turn in Y then −0.025 block per font pixel.
                let m = self.display_matrix(e, pt, t, rs.billboard, &rs.pose(progress))
                    * Mat4::from_rotation_y(PI)
                    * Mat4::from_scale(Vec3::splat(-0.025));
                self.objects.texts.push(TextDraw {
                    id: e.id,
                    m,
                    light: lw,
                    opacity: text.opacity_at(progress) as u8,
                    background: text.background_at(progress),
                    flags: text.flags,
                    label: u32::MAX,
                    hand: false,
                });
                true
            }
            _ => return None,
        })
    }

    /// Item of a display: network context (meta 24) then half-turn in Y.
    fn push_display_item(
        &mut self,
        e: &Entity,
        models: &rc_assets::item::ItemModels,
        stack: &ItemStack,
        context: i8,
        m: Mat4,
        lw: u32,
    ) -> bool {
        let m = m * Mat4::from_rotation_y(PI);
        self.push_stack(models, stack, display_context(context), Some(e), m, lw)
    }

    /// Sign orientation (constraint frozen at the tick) then interpolated transformation of the
    /// display entities (§ 13.3, 13.5).
    fn display_matrix(&self, e: &Entity, pt: f32, t: Mat4, billboard: i8, pose: &DisplayPose) -> Mat4 {
        let yaw = rot_lerp(pt, e.yaw_o, e.yaw);
        let pitch = e.pitch_o + (e.pitch - e.pitch_o) * pt;
        // Camera: towards the camera = 3rd column of the orientation.
        let back = self.cam_rot.z_axis.truncate();
        let cam_yaw = (-back.x).atan2(back.z).to_degrees() + 180.0;
        let cam_pitch = back.y.asin().to_degrees();
        let (psi, theta) = match billboard {
            1 => (cam_yaw - 180.0, pitch),
            2 => (yaw, -cam_pitch),
            3 => (cam_yaw - 180.0, -cam_pitch),
            _ => (yaw, pitch),
        };
        let r = Mat4::from_rotation_y(deg(-psi)) * Mat4::from_rotation_x(deg(theta));
        t * r * pose.matrix()
    }

    /// Orb quad (−0.5, −0.25)…(0.5, 0.75) with another texture, cutout.
    fn push_quad(&mut self, m: Mat4, rect: [f32; 4], pass: usize, light: u32) {
        let base = self.mats.len() as u32;
        self.mats.push(PartMat::new(&m));
        self.instances[pass].push((1, 0, 6, Instance { base, rect, color: 0xffff_ffff, light, overlay: NEUTRAL }));
    }

    /// Resolved layers of the kind (layers of `layers`), same matrix and same animation.
    fn push_layers(&mut self, e: &Entity, m: Mat4, light: u32, anim: impl Fn(&mut Pose<'_>)) -> bool {
        let skin = match e.kind.as_str() {
            "wither_skull" => u32::from(e.meta_bool(8)),
            "cushion" => e.meta_int(8).unwrap_or(0).clamp(0, 15) as u32,
            _ => 0,
        };
        let li = self.resolve(e.kind_id, &e.kind, false, skin);
        if self.arena[li].is_empty() {
            return false;
        }
        for i in 0..self.arena[li].len() {
            let r = self.arena[li][i];
            self.push_resolved(r, m, light, NEUTRAL, 0xffff_ffff, None, &anim);
        }
        true
    }

    /// One baked layer: definition pose animated by `anim`, part matrices, instance.
    #[allow(clippy::too_many_arguments)]
    fn push_resolved(
        &mut self,
        r: Resolved,
        m: Mat4,
        light: u32,
        overlay: u32,
        color: u32,
        pass: Option<usize>,
        anim: impl Fn(&mut Pose<'_>),
    ) {
        let (parts_buf, vis_buf, mut world_m) = std::mem::take(&mut self.scratch);
        let gl = &self.layers[r.layer];
        let mut pose = Pose::reset(&gl.baked, parts_buf, vis_buf);
        anim(&mut pose);
        let base = self.mats.len() as u32;
        world_m.clear();
        world_m.resize(gl.baked.parts.len(), None);
        for (i, part) in gl.baked.parts.iter().enumerate() {
            let parent = if part.parent == u32::MAX { Some(m) } else { world_m[part.parent as usize] };
            let pm = parent.filter(|_| pose.visible[i]).map(|p| p * part_matrix(&pose.parts[i]));
            world_m[i] = pm;
            self.mats.push(match pm {
                Some(pm) => PartMat::new(&pm),
                None => PartMat::zeroed(),
            });
        }
        let pass = pass.unwrap_or_else(|| r.pass.index());
        let (light, overlay) = if r.pass == Pass::Eyes { (0xf0f0, NEUTRAL) } else { (light, overlay) };
        self.instances[pass].push((0, gl.first, gl.count, Instance { base, rect: r.rect, color, light, overlay }));
        self.scratch = (pose.parts, pose.visible, world_m);
    }

    /// Item id by name (cached).
    pub(super) fn item_id(&mut self, name: &'static str) -> Option<i32> {
        if let Some(&i) = self.objects.item_ids.get(name) {
            return (i >= 0).then_some(i);
        }
        let i = self
            .source
            .registry("minecraft:item")
            .and_then(|r| r.entries.iter().position(|n| n == name))
            .map_or(-1, |i| i as i32);
        self.objects.item_ids.insert(name, i);
        (i >= 0).then_some(i)
    }

    /// Stack of meta `index` (otherwise the item `default`), model in context `ctx`.
    #[allow(clippy::too_many_arguments)]
    fn push_meta_stack(
        &mut self,
        e: &Entity,
        index: u8,
        default: &'static str,
        models: &rc_assets::item::ItemModels,
        ctx: usize,
        m: Mat4,
        light: u32,
    ) -> bool {
        match e.meta.get(&index) {
            Some(Meta::Item(Some(stack))) if stack.count > 0 => {
                self.push_stack(models, stack, Some(ctx), Some(e), m, light)
            }
            _ => {
                let Some(item) = self.item_id(default) else { return false };
                let stack = ItemStack { item, count: 1, added: Vec::new(), removed: Vec::new() };
                self.push_stack(models, &stack, Some(ctx), Some(e), m, light)
            }
        }
    }

    /// Item model of a stack: context transformation (`None`: centring only), then
    /// local transformations of the layers. `owner`: non-living entity carrying the stack
    /// (item display, projectile, rocket); its needles read the world, its position and its
    /// rotation, seed = its id. Without a carrier (blocks): no world, seed 0.
    pub(crate) fn push_stack(
        &mut self,
        models: &rc_assets::item::ItemModels,
        stack: &ItemStack,
        ctx: Option<usize>,
        owner: Option<&Entity>,
        m: Mat4,
        light: u32,
    ) -> bool {
        self.push_stack_owned(models, stack, ctx, owner.map(|e| (e.id, e.pos, e.yaw)), m, light)
    }

    /// [`Self::push_stack`] with a non-living carrier given by (id, position, visual rotation
    /// in degrees): frame, whose position and rotation are recomputed.
    fn push_stack_owned(
        &mut self,
        models: &rc_assets::item::ItemModels,
        stack: &ItemStack,
        ctx: Option<usize>,
        owner: Option<(i32, [f64; 3], f32)>,
        m: Mat4,
        light: u32,
    ) -> bool {
        let source = self.source.clone();
        let info = usize::try_from(stack.item).ok().and_then(|i| source.items.get(i));
        let view =
            crate::stack::StackRef { stack, info, ids: &self.ids, data: &source, hold: crate::stack::Hold::NONE };
        let Some(model) = view.model() else { return false };
        let name = usize::try_from(stack.item)
            .ok()
            .and_then(|i| items_in(&source, self.item_idx())?.get(i))
            .map_or("", String::as_str);
        let ctx_name = ctx.map_or("none", |c| rc_assets::model::DISPLAY_CONTEXTS[c]);
        let (mut layers, mut key) = std::mem::take(&mut self.held_scratch);
        layers.clear();
        key.clear();
        match owner {
            Some((id, pos, yaw)) => {
                let env = crate::stack::OwnerEnv::new(&self.world, ctx_name, id, pos, yaw);
                models.layers(name, &model, &view, &env, env.seed(), &mut layers, &mut key);
            }
            None => {
                models.layers(name, &model, &view, &HeldEnv(ctx_name), 0, &mut layers, &mut key);
            }
        }
        let drawn = !layers.is_empty();
        for l in &layers {
            let d = match ctx {
                Some(c) => {
                    let b = models.baked(l.model).unwrap_or(&models.missing);
                    Mat4::from_cols_array_2d(&b.displays[c]).transpose()
                }
                None => Mat4::from_translation(Vec3::splat(-0.5)),
            };
            let mtx = m * d * Mat4::from_cols_array_2d(&l.local).transpose();
            self.push_item_layer(models, l, mtx, light | context_glint_bits(ctx));
        }
        self.held_scratch = (layers, key);
        drawn
    }

    /// Default state of a block by name (cached).
    fn default_state(&mut self, name: &'static str) -> u32 {
        if let Some(&s) = self.objects.block_states.get(name) {
            return s;
        }
        let s = self.source.blocks.iter().find(|b| b.name == name).map_or(0, |b| b.default_state);
        self.objects.block_states.insert(name, s);
        s
    }

    /// Furnace facing north, lit or not.
    fn furnace_state(&mut self, lit: bool) -> u32 {
        let key = if lit { "#furnace_lit" } else { "#furnace" };
        if let Some(&s) = self.objects.block_states.get(key) {
            return s;
        }
        let want = if lit { "true" } else { "false" };
        let s = self
            .source
            .blocks
            .iter()
            .find(|b| b.name == "minecraft:furnace")
            .and_then(|b| {
                (b.first_state..b.first_state + b.state_count).find(|&id| {
                    let mut props = self.source.state_properties(id);
                    props.all(|(n, v)| match n {
                        "facing" => v == "north",
                        "lit" => v == want,
                        _ => true,
                    })
                })
            })
            .unwrap_or(0);
        self.objects.block_states.insert(key, s);
        s
    }

    /// Mesh of a block state (variant drawn with the fixed seed 42, all faces).
    fn block_mesh(&mut self, state: u32) -> Option<usize> {
        if let Some(&i) = self.objects.block_meshes.get(&state) {
            return Some(i);
        }
        let models = self.objects.blocks.clone()?;
        let sm = models.states.get(state as usize)?;
        let mut rng = rc_assets::bake::Lcg::new(42);
        let mut picked: Vec<u32> = Vec::new();
        for part in &sm.parts {
            let (start, count) = if part.options.len() == 1 {
                (part.options[0].1, part.options[0].2)
            } else {
                let mut pick = rng.next_int(part.total_weight);
                let mut chosen = part.options[0];
                for &o in &part.options {
                    if pick < o.0 {
                        chosen = o;
                        break;
                    }
                    pick -= o.0;
                }
                (chosen.1, chosen.2)
            };
            picked.extend(start..start + count);
        }
        let mut tints: Vec<i32> = picked.iter().map(|&q| models.quads[q as usize].tint).collect();
        tints.sort_unstable();
        tints.dedup();
        let mut groups = Vec::new();
        for t in tints {
            let first = self.item_verts.len() as u32;
            for &qi in &picked {
                let q = &models.quads[qi as usize];
                if q.tint != t {
                    continue;
                }
                let rect = self.atlas.rect(&format!("#block/{}", q.texture)).unwrap_or([0.0; 4]);
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
            groups.push((t, first, self.item_verts.len() as u32 - first));
        }
        self.item_dirty = true;
        let translucent = picked.iter().any(|&q| models.quads[q as usize].layer == rc_assets::bake::Layer::Translucent);
        let i = self.objects.meshes.len();
        self.objects.meshes.push(groups);
        self.objects.mesh_translucent.push(translucent);
        self.objects.block_meshes.insert(state, i);
        Some(i)
    }

    /// Mesh of a named block model (`NAMED_MODELS`).
    fn named_mesh(&mut self, name: &'static str) -> Option<usize> {
        if let Some(&i) = self.objects.named_meshes.get(name) {
            return Some(i);
        }
        let quads = self.objects.named.remove(name)?;
        let first = self.item_verts.len() as u32;
        for q in &quads {
            let rect = self.atlas.rect(&q.tex).unwrap_or([0.0; 4]);
            for k in [0, 1, 2, 0, 2, 3] {
                self.item_verts.push(StaticVertex {
                    pos: q.pos[k].map(|c| c * 16.0),
                    uv: [rect[0] + q.uv[k][0] * rect[2], rect[1] + q.uv[k][1] * rect[3]],
                    normal: q.normal,
                    part: 0,
                });
            }
        }
        self.item_dirty = true;
        let i = self.objects.meshes.len();
        self.objects.meshes.push(vec![(-1, first, self.item_verts.len() as u32 - first)]);
        // Named models (frame): opaque type of the game, with the solids.
        self.objects.mesh_translucent.push(false);
        self.objects.named_meshes.insert(name, i);
        Some(i)
    }

    /// ARGB colour of tint layer `t` (tint index of a quad, −1 without tint) of a
    /// state: position-less colour of its source, or biome colour (`World`) depending on the kind of
    /// the source; white without source or client facts.
    pub(super) fn block_tint(&self, state: u32, t: i32, tint: BlockTint) -> u32 {
        let white = 0xffff_ffff;
        let Some(layer) = usize::try_from(t).ok().filter(|&l| l < 4) else { return white };
        let Some(cs) = self.source.client_states.get(state as usize) else { return white };
        let c = match tint {
            BlockTint::Entity => match cs.entity_tint[layer] {
                rc_data::tint::GRASS_DEFAULT => self.objects.grass_default,
                c => c,
            },
            BlockTint::World(biome) => match cs.tint_kind[layer] {
                rc_data::tint::CONSTANT => cs.tint_value[layer],
                k @ (rc_data::tint::GRASS
                | rc_data::tint::FOLIAGE
                | rc_data::tint::DRY_FOLIAGE
                | rc_data::tint::WATER) => biome[usize::from(k - rc_data::tint::GRASS)],
                _ => white,
            },
        };
        0xff00_0000 | c
    }

    /// Baked mesh with the given matrix (block cube from 0 to 1 from the origin); `tint`:
    /// state and colours of the tinted groups (none for the named models).
    fn push_mesh(&mut self, mesh: usize, m: Mat4, light: u32, overlay: u32, tint: Option<(u32, BlockTint)>) {
        let base = self.mats.len() as u32;
        self.mats.push(PartMat::new(&m));
        // Carried blocks, falls and frames: block types of the game, back faces culled. A block
        // with a translucent quad is blended whole in the translucent phase (with the
        // items, sorted by the cube's centre); otherwise cut out with the solids.
        let (pass, src) = if self.objects.mesh_translucent.get(mesh).copied().unwrap_or(false) {
            (super::PASS_ITEM_TRANSLUCENT, 1 | super::SRC_BLOCK_CENTER)
        } else {
            (super::PASS_CUTOUT_CULL, 1)
        };
        for g in 0..self.objects.meshes[mesh].len() {
            let (t, first, count) = self.objects.meshes[mesh][g];
            let c = tint.map_or(0xffff_ffff, |(state, b)| self.block_tint(state, t, b));
            // ARGB → RGBA bytes of the instance.
            let color = (c >> 16 & 0xff) | (c & 0xff00) | (c & 0xff) << 16 | (c & 0xff00_0000);
            self.instances[pass].push((
                src,
                first,
                count,
                Instance { base, rect: [0.0, 0.0, 1.0, 1.0], color, light, overlay },
            ));
        }
    }

    /// Block "as an item" (§ 13.1) with position-less colours (carried, displayed, in a
    /// minecart, TNT).
    pub(super) fn push_block(&mut self, state: u32, m: Mat4, light: u32, overlay: u32) -> bool {
        self.push_block_tinted(state, m, light, overlay, BlockTint::Entity)
    }

    /// Block "as an item": light combined with the block's emission, full light for an
    /// emissive block; state model with colours `tint`, then, for an item only
    /// (`BlockTint::Entity`), special model of the block if it has one (chests, heads, banners,
    /// Shulker boxes…), drawn without block-entity data.
    pub(super) fn push_block_tinted(&mut self, state: u32, m: Mat4, light: u32, overlay: u32, tint: BlockTint) -> bool {
        let info = self.source.state(state).map(|i| (i.light, i.light_emission));
        let light = match info {
            Some((bits, _)) if bits & rc_data::light_bits::EMISSIVE != 0 => 0xf0f0,
            Some((_, emission)) => {
                let b = (light & 0xff).max(u32::from(emission) * 16);
                (light & 0xff00) | b
            }
            None => light,
        };
        let mesh = self.block_mesh(state);
        if let Some(mesh) = mesh {
            self.push_mesh(mesh, m, light, overlay, Some((state, tint)));
        }
        // Moving block (falling, pushed, transient), with world colours: the game only
        // places its state model, never the special model.
        let special = matches!(tint, BlockTint::Entity) && self.push_block_special(state, m, light);
        special || mesh.is_some()
    }

    /// Block entity of state `state` without data (kind, variant, properties read from the state),
    /// if it has a special model drawn outside the world; cached per state.
    fn special_of(&mut self, state: u32) -> Option<crate::block_entities::BlockEntity> {
        use crate::block_entities::{Anim, BlockEntity, Data, Kind, Props, carrier};
        let entry = match self.objects.specials.get(&state) {
            Some(e) => *e,
            None => {
                let c = self.source.block_of(state).and_then(|b| carrier(b.name.strip_prefix("minecraft:")?));
                let e = c
                    .filter(|c| {
                        matches!(
                            c.kind,
                            Kind::Chest
                                | Kind::TrappedChest
                                | Kind::EnderChest
                                | Kind::ShulkerBox
                                | Kind::Bell
                                | Kind::Banner
                                | Kind::Skull
                                | Kind::EnchantingTable
                                | Kind::DecoratedPot
                                | Kind::CopperGolemStatue
                                | Kind::Conduit
                                | Kind::EndPortal
                                | Kind::EndGateway
                        )
                    })
                    .map(|c| (c.kind, c.sub, c.wall, Props::read(&self.source, state)));
                self.objects.specials.insert(state, e);
                e
            }
        };
        let (kind, sub, wall, props) = entry?;
        Some(BlockEntity { kind, sub, wall, state, props, data: Data::None, rev: 0, anim: Anim::default() })
    }

    /// Special model of a block drawn as an entity (`m`: block cube), like the game without
    /// block-entity data: closed chest (type and orientation of the state, Christmas), closed
    /// Shulker box, still bell, banner without pattern or waving, head at rest, closed book
    /// of the enchanting table, pot without sherd, statue, conduit shell, End portal
    /// cube. False if the block has none.
    fn push_block_special(&mut self, state: u32, m: Mat4, light: u32) -> bool {
        use crate::block_entities::Kind;
        let Some(be) = self.special_of(state) else { return false };
        match be.kind {
            Kind::Chest | Kind::TrappedChest | Kind::EnderChest => self.push_chest_special(&be, m, light),
            Kind::ShulkerBox => self.push_shulker(&be, m, light, 0.0),
            Kind::Bell => self.push_bell(&be, m, light, 0.0),
            Kind::CopperGolemStatue => self.push_statue(&be, m, light),
            Kind::Conduit => self.push_conduit(&be, m, light, 0.0),
            Kind::EnchantingTable => {
                // Closed book: (0.5, 0.8125, 0.5), half-turn in Z then quarter turn in X.
                let book = m
                    * Mat4::from_translation(Vec3::new(0.5, 0.8125, 0.5))
                    * Mat4::from_rotation_z(PI)
                    * Mat4::from_rotation_x(PI / 2.0);
                self.push_book(book, light, 0.0, 0.0, 0.0, 0.0)
            }
            Kind::EndPortal => {
                // Reduced cube: y from 0.375 to 0.75.
                let cube = m
                    * Mat4::from_translation(Vec3::new(0.0, 0.375, 0.0))
                    * Mat4::from_scale(Vec3::new(1.0, 0.375, 1.0));
                self.push_portal_cube(cube, 15);
                true
            }
            Kind::EndGateway => {
                self.push_portal_cube(m, 16);
                true
            }
            // Banner (zero waving phase), head (zero animation), pot (without wobble):
            // a neutral render frame (time 0, position 0); skulls shift towards the
            // camera around the eye of the frame.
            Kind::Banner | Kind::Skull | Kind::DecoratedPot => {
                let none = |_: i32, _: i32, _: i32| (0u8, 0u8);
                let all = |_: [f32; 3], _: [f32; 3]| true;
                let air = |_: i32, _: i32, _: i32| 0u32;
                let plain = |_: i32, _: i32, _: i32| [0xff_ffff; 4];
                let f = super::BlockFrame {
                    pt: 0.0,
                    origin: [0, 0],
                    cam: self.eye.to_array().map(f64::from),
                    game_time: 0,
                    min_y: 0,
                    light: &none,
                    visible: &all,
                    items: None,
                    render_distance: 0,
                    top: 0,
                    state: &air,
                    scoping: false,
                    biome_tints: &plain,
                };
                match be.kind {
                    Kind::Banner => self.push_banner([0; 3], &be, m, light, &f),
                    Kind::Skull => self.push_skull(&be, m, light, &f),
                    _ => self.push_pot(&be, m, light, &f),
                }
            }
            _ => false,
        }
    }

    /// Chest without data: half depending on the state's type, state orientation, closed lid;
    /// texture of the kind (copper by oxidation, Christmas for the chest and trapped chest).
    fn push_chest_special(&mut self, be: &crate::block_entities::BlockEntity, m: Mat4, light: u32) -> bool {
        use crate::block_entities::Kind;
        if self.blocks.christmas.is_none() {
            self.blocks.christmas = Some(super::blocks::is_christmas());
        }
        let half = if be.kind == Kind::EnderChest { 0 } else { usize::from(be.props.chest.min(2)) };
        let layer =
            ["minecraft:chest#main", "minecraft:double_chest_left#main", "minecraft:double_chest_right#main"][half];
        let sort = match be.kind {
            Kind::EnderChest => 7,
            Kind::Chest if be.sub > 0 => usize::from(be.sub.min(4)),
            _ if self.blocks.christmas == Some(true) => 6,
            Kind::TrappedChest => 5,
            _ => 0,
        };
        let m = m
            * Mat4::from_translation(Vec3::new(0.5, 0.0, 0.5))
            * Mat4::from_rotation_y(deg(-super::blocks::y_rot(be.props.facing)))
            * Mat4::from_translation(Vec3::new(-0.5, 0.0, -0.5));
        let sprite = super::blocks::CHEST_SPRITES[sort][half];
        self.push_named(layer, sprite, m, super::PASS_CUTOUT_CULL, 0xffff_ffff, light, |_| {})
    }

    /// End portal cube (6 faces, frame `m` of the unit cube), effect `layers` (15 portal,
    /// 16 gateway): camera-relative vertices, taken over into the frame's portals.
    fn push_portal_cube(&mut self, m: Mat4, layers: u32) {
        use crate::block_fx::PortalVertex;
        let v =
            |x: f32, y: f32, z: f32| PortalVertex { pos: m.transform_point3(Vec3::new(x, y, z)).to_array(), layers };
        let faces = [
            [v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(1.0, 0.0, 1.0), v(0.0, 0.0, 1.0)],
            [v(0.0, 1.0, 0.0), v(0.0, 1.0, 1.0), v(1.0, 1.0, 1.0), v(1.0, 1.0, 0.0)],
            [v(0.0, 0.0, 0.0), v(0.0, 1.0, 0.0), v(1.0, 1.0, 0.0), v(1.0, 0.0, 0.0)],
            [v(0.0, 0.0, 1.0), v(1.0, 0.0, 1.0), v(1.0, 1.0, 1.0), v(0.0, 1.0, 1.0)],
            [v(0.0, 0.0, 0.0), v(0.0, 0.0, 1.0), v(0.0, 1.0, 1.0), v(0.0, 1.0, 0.0)],
            [v(1.0, 0.0, 0.0), v(1.0, 1.0, 0.0), v(1.0, 1.0, 1.0), v(1.0, 0.0, 1.0)],
        ];
        for q in faces {
            self.objects.portals.extend([0, 1, 2, 0, 2, 3].map(|k| q[k]));
        }
    }

    /// Frame (§ 3): block model of the frame, then the carried item in `fixed` context.
    fn push_frame(
        &mut self,
        e: &Entity,
        t: Mat4,
        light: &dyn Fn(i32, i32, i32) -> (u8, u8),
        p: [f64; 3],
        items: Option<&rc_assets::item::ItemModels>,
    ) -> bool {
        let glow = e.kind == "glow_item_frame";
        let dir = e.attach_dir();
        let (sx, sy, sz) = match dir {
            0 => (0.0, -1.0, 0.0),
            1 => (0.0, 1.0, 0.0),
            2 => (0.0, 0.0, -1.0),
            4 => (-1.0, 0.0, 0.0),
            5 => (1.0, 0.0, 0.0),
            _ => (0.0, 0.0, 1.0),
        };
        let rot = match dir {
            0 | 1 => Mat4::from_rotation_x(deg(-90.0 * sy)) * Mat4::from_rotation_y(PI),
            2 => Mat4::IDENTITY,
            4 => Mat4::from_rotation_y(deg(180.0 - 90.0)),
            5 => Mat4::from_rotation_y(deg(180.0 - 270.0)),
            _ => Mat4::from_rotation_y(deg(180.0)),
        };
        let m = t * Mat4::from_translation(Vec3::new(sx, sy, sz) * 0.46875) * rot;
        let (b, s) = light(p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32);
        let b = if glow { b.max(5) } else { b };
        let frame_light = pack(b, s);
        let invisible = e.flags() & 0x20 != 0;
        let mut drawn = false;
        // Map whose data is known: model `map=true`, map over the whole face.
        let map = match e.meta.get(&9) {
            Some(Meta::Item(Some(stack))) if stack.count > 0 => self.known_map(stack),
            _ => None,
        };
        if !invisible {
            let name = NAMED_MODELS[usize::from(glow) * 2 + usize::from(map.is_some())];
            if let Some(mesh) = self.named_mesh(name) {
                // Frame model shifted towards the back (the item or map passes in front).
                let lay = self.layering_forward;
                self.push_mesh(mesh, lay * m * Mat4::from_translation(Vec3::splat(-0.5)), frame_light, NEUTRAL, None);
                drawn = true;
            }
        }
        let m = m * Mat4::from_translation(Vec3::new(0.0, 0.0, if invisible { 0.5 } else { 0.4375 }));
        if let Some(id) = map {
            let r = e.meta_int(10).unwrap_or(0).rem_euclid(4);
            let m = m
                * Mat4::from_rotation_z(deg(r as f32 * 90.0 + 180.0))
                * Mat4::from_scale(Vec3::splat(1.0 / 128.0))
                * Mat4::from_translation(Vec3::new(-64.0, -64.0, -1.0));
            // Glow frame: raw coordinates (block 210, sky 240).
            let map_light = if glow { 210 | 240 << 8 } else { frame_light };
            return self.push_map_contents(m, id, map_light, false, true) || drawn;
        }
        if let (Some(models), Some(Meta::Item(Some(stack)))) = (items, e.meta.get(&9))
            && stack.count > 0
        {
            let r = e.meta_int(10).unwrap_or(0);
            let m = m * Mat4::from_rotation_z(deg(r as f32 * 45.0)) * Mat4::from_scale(Vec3::splat(0.5));
            let item_light = if glow { 0xf0f0 } else { frame_light };
            // Carrier = the frame: position recomputed (block centre moved back towards the wall) and
            // visual rotation (hanging direction and item rotation) for the needles.
            let b = e.pos.map(f64::floor);
            let s = [sx, sy, sz].map(|v: f32| f64::from(v) * 0.46875);
            let pos = [b[0] + 0.5 - s[0], b[1] + 0.5 - s[1], b[2] + 0.5 - s[2]];
            let owner = (e.id, pos, frame_visual_yaw(dir, r));
            drawn |= self.push_stack_owned(models, stack, Some(7), Some(owner), m, item_light);
        }
        drawn
    }

    /// Lightning bolt (§ 11): mesh per seed in a fixed slot of the mesh buffer,
    /// white texel tinted (0.45, 0.45, 0.5, 0.3), additive blend.
    fn push_bolt(&mut self, e: &Entity, t: Mat4) -> bool {
        // Seed drawn from the UUID (the game draws it at random client-side).
        let seed = (e.uuid as u64) ^ ((e.uuid >> 64) as u64);
        let first = match self.objects.bolts.iter().find(|s| s.0 == seed) {
            Some(&(_, f)) => f,
            None => {
                if !self.atlas.contains("#white") {
                    self.atlas.insert("#white", Image { width: 1, height: 1, rgba: vec![255; 4] });
                }
                let Some(w) = self.atlas.rect("#white") else { return false };
                let uv = [w[0] + w[2] * 0.5, w[1] + w[3] * 0.5];
                let verts: Vec<StaticVertex> = bolt_quads(seed)
                    .iter()
                    .flat_map(|q| [0, 1, 2, 0, 2, 3].map(|k| q[k]))
                    .map(|p| StaticVertex { pos: p.map(|c| c * 16.0), uv, normal: [0.0, 1.0, 0.0], part: 0 })
                    .collect();
                let first = if self.objects.bolts.len() < BOLT_SLOTS {
                    let f = self.item_verts.len() as u32;
                    self.item_verts.extend_from_slice(&verts);
                    self.objects.bolts.push((seed, f));
                    f
                } else {
                    let slot = self.objects.bolt_next % BOLT_SLOTS;
                    self.objects.bolt_next += 1;
                    let f = self.objects.bolts[slot].1;
                    self.item_verts[f as usize..(f + BOLT_VERTS) as usize].copy_from_slice(&verts);
                    self.objects.bolts[slot] = (seed, f);
                    f
                };
                self.item_dirty = true;
                first
            }
        };
        let base = self.mats.len() as u32;
        self.mats.push(PartMat::new(&t));
        // RGBA (bytes): 0.45, 0.45, 0.5, 0.3.
        let color = 115 | 115 << 8 | 128 << 16 | 77 << 24;
        self.instances[4].push((
            1,
            first,
            BOLT_VERTS,
            Instance { base, rect: [0.0, 0.0, 1.0, 1.0], color, light: 0xf0f0, overlay: NEUTRAL },
        ));
        true
    }

    /// Boat or raft (§ 1.3, 1.4), water mask included.
    fn push_boat(&mut self, e: &Entity, pt: f32, t: Mat4, light: u32) -> bool {
        let yaw = rot_lerp(pt, e.yaw_o, e.yaw);
        let mut m = t * Mat4::from_translation(Vec3::new(0.0, 0.375, 0.0)) * Mat4::from_rotation_y(deg(180.0 - yaw));
        m *= hurt_shake(e, pt);
        // Bubble column tilt: non-normalised quaternion (sin b/2, 0, sin b/2, cos b/2).
        let b = deg(e.extra[6] + (e.extra[5] - e.extra[6]) * pt);
        if b.abs() > 1e-5 {
            let h = (b / 2.0).tan();
            let angle = 2.0 * (std::f32::consts::SQRT_2 * h).atan();
            let k = 1.0 + (b / 2.0).sin().powi(2);
            m *= Mat4::from_axis_angle(Vec3::new(1.0, 0.0, 1.0).normalize(), angle) * Mat4::from_scale(Vec3::splat(k));
        }
        m *= Mat4::from_scale(Vec3::new(-1.0, -1.0, 1.0)) * Mat4::from_rotation_y(deg(90.0));
        let paddles = [e.extra[0], e.extra[1]];
        let active = [e.meta_bool(11), e.meta_bool(12)];
        // Water mask (boats other than rafts): the water surface is not drawn inside the hull.
        if !e.kind.ends_with("_raft") {
            let patch = *self
                .objects
                .water_patch
                .get_or_insert_with(|| self.layer_index.get("minecraft:boat#water_patch").copied());
            if let Some(layer) = patch {
                let r = Resolved { layer, rect: [0.0, 0.0, 1.0, 1.0], pass: Pass::Cutout, wool: Wool::None };
                self.push_resolved(r, m, light, NEUTRAL, 0xffff_ffff, Some(3), |_| {});
            }
        }
        self.push_layers(e, m, light, |p| {
            for (side, name) in ["left_paddle", "right_paddle"].into_iter().enumerate() {
                let t = if active[side] { paddles[side] - PI / 8.0 + pt * PI / 8.0 } else { 0.0 };
                let lerp = |f: f32, a: f32, b: f32| a + f.clamp(0.0, 1.0) * (b - a);
                let xr = lerp(((-t).sin() + 1.0) / 2.0, -PI / 3.0, -PI / 12.0);
                let mut yr = lerp(((-t + 1.0).sin() + 1.0) / 2.0, -PI / 4.0, PI / 4.0);
                if side == 1 {
                    yr = PI - yr;
                }
                p.set(name, |x| {
                    x[3] = xr;
                    x[4] = yr;
                });
            }
        })
    }

    /// Exits of a rail (§ 2.3, rail probe); `None` if the state is not a rail.
    fn rail_exits(&self, state: u32) -> Option<([i32; 3], [i32; 3])> {
        let name = &self.source.block_of(state)?.name;
        if !name.ends_with("rail") {
            return None;
        }
        let shape = self.source.state_properties(state).find(|(n, _)| *n == "shape")?.1;
        let (n, s, w, ea) = ([0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]);
        let down = |v: [i32; 3]| [v[0], -1, v[2]];
        Some(match shape {
            "north_south" => (n, s),
            "east_west" => (w, ea),
            "ascending_east" => (down(w), ea),
            "ascending_west" => (w, down(ea)),
            "ascending_north" => (n, down(s)),
            "ascending_south" => (down(n), s),
            "south_east" => (s, ea),
            "south_west" => (s, w),
            "north_west" => (n, w),
            "north_east" => (n, ea),
            _ => return None,
        })
    }

    /// Point on the nearest rail (probe of the old behaviour).
    fn rail_probe(&self, world: &dyn Fn(i32, i32, i32) -> u32, x: f64, y: f64, z: f64) -> Option<[f64; 3]> {
        let (xt, mut yt, zt) = (x.floor() as i32, y.floor() as i32, z.floor() as i32);
        if self.rail_exits(world(xt, yt - 1, zt)).is_some() {
            yt -= 1;
        }
        let (e0, e1) = self.rail_exits(world(xt, yt, zt))?;
        let at = |e: [i32; 3]| {
            [
                f64::from(xt) + 0.5 + f64::from(e[0]) * 0.5,
                f64::from(yt) + 0.0625 + f64::from(e[1]) * 0.5,
                f64::from(zt) + 0.5 + f64::from(e[2]) * 0.5,
            ]
        };
        let (p0, p1) = (at(e0), at(e1));
        let d = [p1[0] - p0[0], 2.0 * (p1[1] - p0[1]), p1[2] - p0[2]];
        let k = if d[0] == 0.0 {
            z - f64::from(zt)
        } else if d[2] == 0.0 {
            x - f64::from(xt)
        } else {
            ((x - p0[0]) * d[0] + (z - p0[2]) * d[2]) * 2.0
        };
        let mut out = [p0[0] + d[0] * k, p0[1] + d[1] * k, p0[2] + d[2] * k];
        if d[1] < 0.0 {
            out[1] += 1.0;
        } else if d[1] > 0.0 {
            out[1] += 0.5;
        }
        Some(out)
    }

    /// Probe offset by `eps` along the rail.
    fn rail_probe_offset(
        &self,
        world: &dyn Fn(i32, i32, i32) -> u32,
        x: f64,
        y: f64,
        z: f64,
        eps: f64,
    ) -> Option<[f64; 3]> {
        let (xt, mut yt, zt) = (x.floor() as i32, y.floor() as i32, z.floor() as i32);
        if self.rail_exits(world(xt, yt - 1, zt)).is_some() {
            yt -= 1;
        }
        let (e0, e1) = self.rail_exits(world(xt, yt, zt))?;
        let mut y = f64::from(yt);
        if e0[1] != 0 || e1[1] != 0 {
            y += 1.0;
        }
        let (ux, uz) = (f64::from(e1[0] - e0[0]), f64::from(e1[2] - e0[2]));
        let len = (ux * ux + uz * uz).sqrt();
        let (x, z) = (x + ux / len * eps, z + uz / len * eps);
        let cell = |e: [i32; 3]| x.floor() as i32 - xt == e[0] && z.floor() as i32 - zt == e[2];
        if e0[1] != 0 && cell(e0) {
            y += f64::from(e0[1]);
        } else if e1[1] != 0 && cell(e1) {
            y += f64::from(e1[1]);
        }
        self.rail_probe(world, x, y, z)
    }

    /// Minecart (§ 2.3, old behaviour: rail probe) and its displayed block.
    fn push_minecart(
        &mut self,
        e: &Entity,
        pt: f32,
        t: Mat4,
        p: [f64; 3],
        light: u32,
        world: &dyn Fn(i32, i32, i32) -> u32,
    ) -> bool {
        let k = e.kind.as_str();
        let mut yaw = rot_lerp(pt, e.yaw_o, e.yaw);
        let mut pitch = e.pitch_o + (e.pitch - e.pitch_o) * pt;
        // Minecart's own offset first (two stacked minecarts do not flicker).
        let mut m = t * Mat4::from_translation(minecart_offset(e.id));
        if let Some(c) = self.rail_probe(world, p[0], p[1], p[2]) {
            let f = self.rail_probe_offset(world, p[0], p[1], p[2], 0.3).unwrap_or(c);
            let b = self.rail_probe_offset(world, p[0], p[1], p[2], -0.3).unwrap_or(c);
            m *= Mat4::from_translation(Vec3::new(
                (c[0] - p[0]) as f32,
                ((f[1] + b[1]) / 2.0 - p[1]) as f32,
                (c[2] - p[2]) as f32,
            ));
            let d = Vec3::new((b[0] - f[0]) as f32, (b[1] - f[1]) as f32, (b[2] - f[2]) as f32);
            if d.length_squared() > 0.0 {
                let d = d.normalize();
                yaw = d.z.atan2(d.x).to_degrees();
                // Arctangent in radians × 73 (degrees).
                pitch = d.y.atan() * 73.0;
            }
        }
        let m = m
            * Mat4::from_translation(Vec3::new(0.0, 0.375, 0.0))
            * Mat4::from_rotation_y(deg(180.0 - yaw))
            * Mat4::from_rotation_z(deg(-pitch))
            * hurt_shake(e, pt);
        let (default, offset) = match k {
            "chest_minecart" => (Some(self.default_state("minecraft:chest")), 8),
            "furnace_minecart" => (Some(self.furnace_state(e.meta_bool(13))), 6),
            "hopper_minecart" => (Some(self.default_state("minecraft:hopper")), 1),
            "tnt_minecart" => (Some(self.default_state("minecraft:tnt")), 6),
            "spawner_minecart" => (Some(self.default_state("minecraft:spawner")), 6),
            "command_block_minecart" => (Some(self.default_state("minecraft:command_block")), 6),
            _ => (None, 6),
        };
        let state = meta_state(e, 11).filter(|&s| s != 0).or(default);
        if let Some(state) = state {
            let off = e.meta_int(12).unwrap_or(offset) as f32;
            let mut bm = m
                * Mat4::from_scale(Vec3::splat(0.75))
                * Mat4::from_translation(Vec3::new(-0.5, (off - 8.0) / 16.0, 0.5))
                * Mat4::from_rotation_y(deg(90.0));
            // Primed TNT minecart (local fuse, event 70): swelling and blinking of the
            // last 10 ticks, like TNT.
            let mut overlay = NEUTRAL;
            if k == "tnt_minecart" && e.fuse > -1 {
                let f = e.fuse as f32 - pt + 1.0;
                let (g, on) = tnt_swell(f);
                if f < 10.0 {
                    bm *= Mat4::from_translation(Vec3::new(-g * 0.5, 0.0, -g * 0.5))
                        * Mat4::from_scale(Vec3::splat(1.0 + g));
                }
                if on {
                    overlay = WHITE;
                }
            }
            self.push_block(state, bm, light, overlay);
        }
        self.push_layers(e, m * Mat4::from_scale(Vec3::new(-1.0, -1.0, 1.0)), light, |_| {})
    }
}

// ---------------------------------------------------------------------------------------------
// Display entities: render state frozen at the tick and interpolation of the transformation
// (`entities-objects.md` § 13.5).

/// Data that makes the common render state be redone: translation, scale, left and
/// right rotations, billboard constraint, brightness, shadow radius and strength.
const DISPLAY_RENDER_DATA: [u8; 8] = [11, 12, 13, 14, 15, 16, 18, 19];

/// Transformation of a display decomposed: translation, left rotation, scale, right
/// rotation. Raw quaternions [x, y, z, w] (never normalised).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DisplayPose {
    t: [f32; 3],
    l: [f32; 4],
    s: [f32; 3],
    r: [f32; 4],
}

impl DisplayPose {
    fn from_meta(e: &Entity) -> Self {
        DisplayPose {
            t: meta_vec3(e, 11).unwrap_or([0.0; 3]),
            s: meta_vec3(e, 12).unwrap_or([1.0; 3]),
            l: meta_quat(e, 13).unwrap_or([0.0, 0.0, 0.0, 1.0]),
            r: meta_quat(e, 14).unwrap_or([0.0, 0.0, 0.0, 1.0]),
        }
    }

    /// Blend towards `to`: linear translation and scale, spherical rotations.
    fn blend(&self, to: &Self, p: f32) -> Self {
        let lerp3 = |a: [f32; 3], b: [f32; 3]| std::array::from_fn(|i| (b[i] - a[i]).mul_add(p, a[i]));
        DisplayPose {
            t: lerp3(self.t, to.t),
            l: slerp(self.l, to.l, p),
            s: lerp3(self.s, to.s),
            r: slerp(self.r, to.r, p),
        }
    }

    /// Translation × left rotation × scale × right rotation.
    pub(crate) fn matrix(&self) -> Mat4 {
        Mat4::from_translation(Vec3::from(self.t))
            * quat_matrix(self.l)
            * Mat4::from_scale(Vec3::from(self.s))
            * quat_matrix(self.r)
    }
}

/// Spherical interpolation without normalisation: linear fallback when the two rotations are
/// almost equal (cosine difference < 1e-6), shortest path by the sign of the cosine.
fn slerp(a: [f32; 4], b: [f32; 4], alpha: f32) -> [f32; 4] {
    let cos = a[0].mul_add(b[0], a[1].mul_add(b[1], a[2].mul_add(b[2], a[3] * b[3])));
    let abs = cos.abs();
    let (s0, mut s1) = if 1.0 - abs > 1e-6 {
        let sin2 = 1.0 - abs * abs;
        let inv = 1.0 / sin2.sqrt();
        let omega = (sin2 * inv).atan2(abs);
        (((1.0 - alpha) * omega).sin() * inv, (alpha * omega).sin() * inv)
    } else {
        (1.0 - alpha, alpha)
    };
    if cos < 0.0 {
        s1 = -s1;
    }
    std::array::from_fn(|i| s0.mul_add(a[i], s1 * b[i]))
}

/// Matrix of a quaternion in homogeneous form (squares of w, x, y, z): a non-unit quaternion
/// rotates AND scales by its squared norm.
fn quat_matrix(q: [f32; 4]) -> Mat4 {
    let [x, y, z, w] = q;
    let (w2, x2, y2, z2) = (w * w, x * x, y * y, z * z);
    let (dzw, dxy, dxz, dyw, dyz, dxw) = (2.0 * z * w, 2.0 * x * y, 2.0 * x * z, 2.0 * y * w, 2.0 * y * z, 2.0 * x * w);
    Mat4::from_cols(
        glam::Vec4::new(w2 + x2 - z2 - y2, dxy + dzw, dxz - dyw, 0.0),
        glam::Vec4::new(dxy - dzw, y2 - z2 + w2 - x2, dyz + dxw, 0.0),
        glam::Vec4::new(dyw + dxz, dyz - dxw, z2 - y2 - x2 + w2, 0.0),
        glam::Vec4::W,
    )
}

/// Content specific to the kind, frozen with the common state.
#[derive(Clone, Debug)]
pub(crate) enum DisplaySub {
    Block(u32),
    /// Stack and network display context (meta 24).
    Item(Option<ItemStack>, i8),
    Text(Box<DisplayText>),
}

/// Text of a display frozen at the tick: component (meta 23), line width (24), opacity
/// (26, signed byte, interpolated linearly as an integer), background (25, ARGB interpolated per channel),
/// style flags (27).
#[derive(Clone, Debug)]
pub(crate) struct DisplayText {
    pub(crate) text: Option<rc_proto::nbt::Nbt>,
    pub(crate) line_width: i32,
    opacity: (i32, i32),
    background: (u32, u32),
    pub(crate) flags: i8,
}

/// `a + floor(p × (b − a))`, like the game's integer interpolation.
fn lerp_int(p: f32, a: i32, b: i32) -> i32 {
    a + (p * (b - a) as f32).floor() as i32
}

impl DisplayText {
    /// Text opacity at progress `p` (byte: −1 = 255 = opaque).
    pub(crate) fn opacity_at(&self, p: f32) -> i32 {
        if self.opacity.0 == self.opacity.1 { self.opacity.1 } else { lerp_int(p, self.opacity.0, self.opacity.1) }
    }

    /// ARGB background at progress `p`, channel by channel.
    pub(crate) fn background_at(&self, p: f32) -> u32 {
        let (a, b) = self.background;
        if a == b {
            return b;
        }
        (0..4).fold(0, |acc, k| {
            let s = k * 8;
            let c = lerp_int(p, ((a >> s) & 0xff) as i32, ((b >> s) & 0xff) as i32) as u32;
            acc | (c & 0xff) << s
        })
    }
}

/// Render state frozen at the tick: transformation (start of the interpolation, absent = constant;
/// end), billboard constraint, brightness, shadow (radius then strength: start, end).
#[derive(Clone, Debug)]
pub(crate) struct DisplayRender {
    from: Option<DisplayPose>,
    to: DisplayPose,
    pub(crate) billboard: i8,
    pub(crate) brightness: i32,
    shadow: [(f32, f32); 2],
    pub(crate) sub: DisplaySub,
}

impl DisplayRender {
    /// Transformation at progress `p` (the end from `p` ≥ 1).
    pub(crate) fn pose(&self, p: f32) -> DisplayPose {
        match &self.from {
            Some(from) if p < 1.0 => from.blend(&self.to, p),
            _ => self.to,
        }
    }

    fn shadow_at(&self, p: f32) -> (f32, f32) {
        let lerp = |(a, b): (f32, f32)| a + p * (b - a);
        (lerp(self.shadow[0]), lerp(self.shadow[1]))
    }
}

/// Client tracking of a display: flags set on reception of the data, applied at the next
/// tick; last progress computed at render (start of an interpolation that cuts off
/// another).
#[derive(Clone, Debug)]
pub struct DisplayState {
    new_start: bool,
    new_duration: bool,
    new_render: bool,
    start: i64,
    duration: i32,
    pub(crate) last: std::cell::Cell<f32>,
    pub(crate) render: Option<DisplayRender>,
}

impl Default for DisplayState {
    fn default() -> Self {
        DisplayState {
            new_start: false,
            new_duration: false,
            new_render: false,
            start: i64::from(i32::MIN),
            duration: 0,
            last: std::cell::Cell::new(0.0),
            render: None,
        }
    }
}

impl DisplayState {
    /// Interpolation progress at tick `age` + `pt`, bounded to [0, 1] (1 without duration).
    pub(crate) fn progress(&self, age: u32, pt: f32) -> f32 {
        if self.duration <= 0 {
            return 1.0;
        }
        let since = (i64::from(age) - self.start) as f32 + pt;
        (since / self.duration as f32).clamp(0.0, 1.0)
    }
}

fn is_display(k: &str) -> bool {
    matches!(k, "block_display" | "item_display" | "text_display")
}

/// Data `i` received for entity `e` (already stored in its meta): display flags.
pub(crate) fn display_data(e: &mut Entity, i: u8) {
    if !is_display(&e.kind) {
        return;
    }
    let render = DISPLAY_RENDER_DATA.contains(&i)
        || match e.kind.as_str() {
            "block_display" => i == 23,
            "item_display" => matches!(i, 23 | 24),
            _ => (23..=27).contains(&i),
        };
    let ds = e.display.get_or_insert_with(Default::default);
    match i {
        8 => ds.new_start = true,
        9 => ds.new_duration = true,
        _ => {}
    }
    ds.new_render |= render;
}

/// Client tick of a display (age already advanced): start, duration then render state.
pub(crate) fn display_tick(e: &mut Entity) {
    let Some(mut ds) = e.display.take() else { return };
    let age = i64::from(e.age);
    if std::mem::take(&mut ds.new_start) {
        ds.start = age + i64::from(e.meta_int(8).unwrap_or(0));
    }
    if std::mem::take(&mut ds.new_duration) {
        ds.duration = e.meta_int(9).unwrap_or(0);
    }
    if std::mem::take(&mut ds.new_render) {
        let to = DisplayPose::from_meta(e);
        let radius = meta_float(e, 18).unwrap_or(0.0);
        let strength = meta_float(e, 19).unwrap_or(1.0);
        let (from, shadow) = match &ds.render {
            Some(prev) if ds.duration != 0 => {
                let p = ds.last.get();
                let (r0, s0) = prev.shadow_at(p);
                (Some(prev.pose(p)), [(r0, radius), (s0, strength)])
            }
            _ => (None, [(radius, radius), (strength, strength)]),
        };
        let sub = match e.kind.as_str() {
            "block_display" => DisplaySub::Block(meta_state(e, 23).unwrap_or(0)),
            "item_display" => DisplaySub::Item(
                match e.meta.get(&23) {
                    Some(Meta::Item(s)) => s.clone(),
                    _ => None,
                },
                e.meta_byte(24),
            ),
            _ => {
                let text = match e.meta.get(&23) {
                    Some(Meta::Text(t)) => t.clone(),
                    _ => None,
                };
                let opacity = i32::from(match e.meta.get(&26) {
                    Some(Meta::Byte(b)) => *b,
                    _ => -1,
                });
                let background = e.meta_int(25).unwrap_or(0x4000_0000) as u32;
                // Interpolation from the previous state at the progress of the last render.
                let prev = match &ds.render {
                    Some(DisplayRender { sub: DisplaySub::Text(t), .. }) if ds.duration != 0 => Some(t),
                    _ => None,
                };
                let p = ds.last.get();
                DisplaySub::Text(Box::new(DisplayText {
                    text,
                    line_width: e.meta_int(24).unwrap_or(200),
                    opacity: (prev.map_or(opacity, |t| t.opacity_at(p)), opacity),
                    background: (prev.map_or(background, |t| t.background_at(p)), background),
                    flags: e.meta_byte(27),
                }))
            }
        };
        ds.render = Some(DisplayRender {
            from,
            to,
            billboard: e.meta_byte(15),
            brightness: e.meta_int(16).unwrap_or(-1),
            shadow,
            sub,
        });
    }
    e.display = Some(ds);
}

/// Box (width, height) of a display subject to out-of-view culling; `None` (never culled) while one of the two is 0, which is the default.
pub(crate) fn display_cull_box(e: &Entity) -> Option<(f32, f32)> {
    let (w, h) = (meta_float(e, 20).unwrap_or(0.0), meta_float(e, 21).unwrap_or(0.0));
    (is_display(&e.kind) && w != 0.0 && h != 0.0).then_some((w, h))
}

/// Interpolated shadow (radius, strength) of a display; nothing until the render state exists.
pub(crate) fn display_shadow(e: &Entity, pt: f32) -> Option<(f32, f32)> {
    if !is_display(&e.kind) {
        return None;
    }
    let ds = e.display.as_deref();
    Some(match ds.and_then(|d| d.render.as_ref().map(|r| (d, r))) {
        Some((d, r)) => r.shadow_at(d.progress(e.age, pt)),
        None => (0.0, 0.0),
    })
}

/// What the object passes done outside `prepare` read from the frame.
pub struct ObjectsFrame {
    pub pt: f32,
    /// Integer origin (x, z) of the render frame and camera position.
    pub origin: [i32; 2],
    pub cam: [f64; 3],
    /// Local player: id, left-handed (option `mainHand`), first-person view, field of view
    /// of the option (integer degrees), camera orientation, width / height
    /// ratio of the window.
    pub local: i32,
    pub local_left: bool,
    pub first_person: bool,
    pub fov: i32,
    pub yaw: f32,
    pub pitch: f32,
    pub aspect: f32,
}

/// Left main hand of a player (metadata 15: 0 left, 1 right).
fn player_left_handed(e: &Entity) -> bool {
    // Main hand = humanoid arm serializer (Meta::Variant); 0 = left.
    e.meta_int(15) == Some(0)
}

/// Starting point of the fishing line: hand that holds the rod (main hand if the main item
/// is a rod, otherwise the other). First-person view of the local caster: point
/// of the near plane (±0.525, −0.1) scaled by 960 / field of view, rotated by the swing; otherwise
/// in front of the body at eye height − 0.45 (× scale).
fn fishing_hand(owner: &Entity, local: bool, rod: Option<i32>, f: &ObjectsFrame, table: &[[f32; 4]]) -> [f64; 3] {
    let pt = f.pt;
    let main_left = if local { f.local_left } else { player_left_handed(owner) };
    let holds_rod = owner.equipment[0].as_ref().is_some_and(|s| Some(s.item) == rod);
    let invert = if main_left == holds_rod { -1.0f32 } else { 1.0 };
    let swing = owner.swing_at(pt);
    let swing2 = crate::sky_mesh::table_sin(swing.sqrt() * PI);
    let p = owner.render_pos(pt);
    let size = owner.size(table);
    let eye = [p[0], p[1] + f64::from(size.eye), p[2]];
    if local && f.first_person {
        let fov = f.fov as f32;
        let h = (f64::from(fov * (PI / 180.0)) / 2.0).tan() * 0.05;
        let w = h * f64::from(f.aspect);
        let (right, up, fwd) = crate::camera::axes(f.yaw, f.pitch);
        let (x, y) = (f64::from(invert * 0.525), f64::from(-0.1f32));
        let scale = 960.0 / f64::from(fov);
        let mut v: [f64; 3] = std::array::from_fn(|i| {
            (f64::from(fwd[i]) * 0.05 + f64::from(up[i]) * h * y + f64::from(right[i]) * w * x) * scale
        });
        // Rotations of the game on a vector: around Y (x, z) then around X (y, z).
        let a = swing2 * 0.5;
        let (s, c) = (f64::from(crate::sky_mesh::table_sin(a)), f64::from(crate::sky_mesh::table_cos(a)));
        v = [v[0] * c + v[2] * s, v[1], v[2] * c - v[0] * s];
        let a = -swing2 * 0.7;
        let (s, c) = (f64::from(crate::sky_mesh::table_sin(a)), f64::from(crate::sky_mesh::table_cos(a)));
        v = [v[0], v[1] * c + v[2] * s, v[2] * c - v[1] * s];
        [eye[0] + v[0], eye[1] + v[1], eye[2] + v[2]]
    } else {
        let yaw = rot_lerp(pt, owner.body_o, owner.body) * (PI / 180.0);
        let (sin, cos) = (f64::from(crate::sky_mesh::table_sin(yaw)), f64::from(crate::sky_mesh::table_cos(yaw)));
        let scale = f64::from(owner.scale);
        let right = f64::from(invert) * 0.35 * scale;
        let fwd = 0.8 * scale;
        let y = if owner.pose() == 5 { f64::from(-0.1875f32) } else { 0.0 };
        [eye[0] - cos * right - sin * fwd, eye[1] + y - 0.45 * scale, eye[2] - sin * right + cos * fwd]
    }
}

/// Fishing line: 16 segments of the parabola `(x·a, y·(a² + a)/2 + 0.25, z·a)` starting from the
/// bobber, `o` = hand − (bobber + 0.25 in y).
fn fishing_line(o: [f32; 3]) -> [[f32; 3]; 17] {
    std::array::from_fn(|i| {
        let a = i as f32 / 16.0;
        [o[0] * a, o[1] * (a * a + a) * 0.5 + 0.25, o[2] * a]
    })
}

/// RGBA colour (bytes) of the guardian's beam depending on the attack scale.
/// Offset (x, y) of a guardian's eye towards `look`: down (y 1) if the target is not
/// above the eyes; x = signed root of the dot product between the gaze direction
/// (flat) and the target direction → guardian turned by a quarter turn, × 2.
fn guardian_eye(eye: [f64; 3], look: [f64; 3], pitch: f32, yaw: f32) -> [f32; 2] {
    let y = if look[1] - eye[1] > 0.0 { 0.0 } else { 1.0 };
    let (f, g) = (f64::from(pitch).to_radians(), -f64::from(yaw).to_radians());
    let view = [g.sin() * f.cos(), g.cos() * f.cos()];
    let (dx, dz) = (eye[0] - look[0], eye[2] - look[2]);
    let len = (dx * dx + dz * dz).sqrt();
    let (dx, dz) = if len < 1.0e-5 { (0.0, 0.0) } else { (dx / len, dz / len) };
    // Quarter turn around y: (x, z) → (z, −x).
    let dot = view[0] * dz + view[1] * -dx;
    [(dot.abs() as f32).sqrt() * 2.0 * (dot.signum() as f32), y]
}

fn guardian_beam_color(scale: f32) -> u32 {
    let cs = scale * scale;
    let r = 64 + (cs * 191.0) as i32;
    let g = 32 + (cs * 191.0) as i32;
    let b = 128 - (cs * 64.0) as i32;
    (r as u32 & 0xff) | (g as u32 & 0xff) << 8 | (b as u32 & 0xff) << 16 | 0xff00_0000
}

/// Guardian's beam in frame `m` (guardian's eyes), towards `d`: two crossed planes of
/// radius 0.2 rotating with time, scrolling v, and a 0.282 cap at the end.
fn guardian_beam_quads(m: Mat4, d: Vec3, time: f32, scale: f32, rect: [f32; 4], out: &mut Vec<FreeVertex>) {
    let len = d.length() + 1.0;
    let n = d.normalize_or_zero();
    let x_rot = n.y.clamp(-1.0, 1.0).acos();
    let y_rot = PI / 2.0 - n.z.atan2(n.x);
    let m = m * Mat4::from_rotation_y(y_rot) * Mat4::from_rotation_x(x_rot);
    let rot = time * 0.05 * -1.5;
    let color = guardian_beam_color(scale);
    let normal = m.transform_vector3(Vec3::Y).normalize_or_zero().to_array();
    let tc = crate::sky_mesh::table_cos;
    let ts = crate::sky_mesh::table_sin;
    let at = |a: f32, r: f32| (tc(rot + a) * r, ts(rot + a) * r);
    let (wnx, wnz) = at(PI * 3.0 / 4.0, 0.282);
    let (enx, enz) = at(PI / 4.0, 0.282);
    let (wsx, wsz) = at(PI * 5.0 / 4.0, 0.282);
    let (esx, esz) = at(PI * 7.0 / 4.0, 0.282);
    let (wx, wz) = at(PI, 0.2);
    let (ex, ez) = at(0.0, 0.2);
    let (nx, nz) = at(PI / 2.0, 0.2);
    let (sx, sz) = at(PI * 3.0 / 2.0, 0.2);
    let tex_v_off = time * 0.5 % 1.0;
    let min_v = -1.0 + tex_v_off;
    let max_v = min_v + len * 2.5;
    let vert = |x: f32, y: f32, z: f32, u: f32, v: f32| FreeVertex {
        pos: m.transform_point3(Vec3::new(x, y, z)).to_array(),
        uv: [u, v],
        rect,
        color,
        light: pack(15, 15),
        normal,
    };
    let v_base = if (time.floor() as i32) % 2 == 0 { 0.5 } else { 0.0 };
    let quads = [
        [
            vert(wx, len, wz, 0.4999, max_v),
            vert(wx, 0.0, wz, 0.4999, min_v),
            vert(ex, 0.0, ez, 0.0, min_v),
            vert(ex, len, ez, 0.0, max_v),
        ],
        [
            vert(nx, len, nz, 0.4999, max_v),
            vert(nx, 0.0, nz, 0.4999, min_v),
            vert(sx, 0.0, sz, 0.0, min_v),
            vert(sx, len, sz, 0.0, max_v),
        ],
        [
            vert(wnx, len, wnz, 0.5, v_base + 0.5),
            vert(enx, len, enz, 1.0, v_base + 0.5),
            vert(esx, len, esz, 1.0, v_base),
            vert(wsx, len, wsz, 0.5, v_base),
        ],
    ];
    for q in quads {
        out.extend([0, 1, 2, 2, 3, 0].map(|k| q[k]));
    }
}

impl EntityRenderer {
    /// Text displays placed by the last `prepare`.
    pub fn text_draws(&self) -> &[TextDraw] {
        &self.objects.texts
    }

    /// Before `prepare`: bobbers with a known owner (the others are not drawn) and
    /// fishing lines of the frame.
    pub fn objects_prepass(&mut self, ents: &crate::entities::Entities, f: &ObjectsFrame) {
        self.objects.texts.clear();
        self.objects.labels.clear();
        self.objects.bobber_owners.clear();
        self.objects.fishers.clear();
        self.objects.local = f.local;
        self.objects.lines.clear();
        self.objects.guardian_eyes.clear();
        for e in ents.map.values().filter(|e| matches!(e.kind.as_str(), "guardian" | "elder_guardian")) {
            let id = e.meta_int(17).unwrap_or(0);
            // Announced attack target (even if absent: eye at rest), otherwise the camera entity.
            let look = if id != 0 { id } else { f.local };
            let Some(t) = crate::misc::entity(ents, look) else { continue };
            let eye_of = |x: &Entity| {
                let p = x.render_pos(f.pt);
                [p[0], p[1] + f64::from(x.size(&self.dims).eye), p[2]]
            };
            let pitch = e.pitch_o + f.pt * (e.pitch - e.pitch_o);
            let yaw = crate::entities::rot_lerp(f.pt, e.head_o, e.head);
            self.objects.guardian_eyes.insert(e.id, guardian_eye(eye_of(e), eye_of(t), pitch, yaw));
        }
        let rod = self.item_id("minecraft:fishing_rod");
        let rel =
            |p: [f64; 3]| [(p[0] - f64::from(f.origin[0])) as f32, p[1] as f32, (p[2] - f64::from(f.origin[1])) as f32];
        for e in ents.map.values().filter(|e| e.kind == "fishing_bobber") {
            let Some(owner) = crate::misc::entity(ents, e.data).filter(|o| o.kind == "player") else { continue };
            self.objects.bobber_owners.insert(e.id);
            self.objects.fishers.insert(owner.id);
            if !self.within_view_distance(e, f.pt, f.cam) {
                continue;
            }
            let hand = fishing_hand(owner, e.data == f.local, rod, f, &self.dims);
            let hook = e.render_pos(f.pt);
            let o = [(hand[0] - hook[0]) as f32, (hand[1] - (hook[1] + 0.25)) as f32, (hand[2] - hook[2]) as f32];
            let base = rel(hook);
            let pts = fishing_line(o);
            for w in pts.windows(2) {
                let a = std::array::from_fn(|i| base[i] + w[0][i]);
                let b = std::array::from_fn(|i| base[i] + w[1][i]);
                self.objects.lines.push((a, b));
            }
        }
    }

    /// After `prepare`: beams of guardians attacking a known living target.
    pub fn push_guardian_beams(&mut self, ents: &crate::entities::Entities, f: &ObjectsFrame) {
        let pt = f.pt;
        for e in ents.map.values().filter(|e| matches!(e.kind.as_str(), "guardian" | "elder_guardian")) {
            let id = e.meta_int(17).unwrap_or(0);
            if id == 0 || !self.within_view_distance(e, pt, f.cam) {
                continue;
            }
            let Some(target) = crate::misc::entity(ents, id).filter(|t| t.class != Kind::Other) else { continue };
            let Some(rect) = self.atlas.rect("entity/guardian/guardian_beam") else { return };
            let dur = if e.kind == "elder_guardian" { 60.0 } else { 80.0 };
            let time = e.extra[5] + pt;
            let p = e.render_pos(pt);
            let eye = f64::from(e.size(&self.dims).eye);
            let q = target.render_pos(pt);
            let th = f64::from(target.size(&self.dims).h) * 0.5;
            let d = Vec3::new((q[0] - p[0]) as f32, (q[1] + th - (p[1] + eye)) as f32, (q[2] - p[2]) as f32);
            let m = Mat4::from_translation(Vec3::new(
                (p[0] - f64::from(f.origin[0])) as f32,
                (p[1] + eye) as f32,
                (p[2] - f64::from(f.origin[1])) as f32,
            ));
            guardian_beam_quads(m, d, time, time / dur, rect, &mut self.free_verts[FREE_GBEAM]);
        }
    }

    /// After the frame's lines have been filled: fishing lines (black, line width
    /// of the window: max(2.5; width / 1920 × 2.5) pixels).
    pub fn push_fishing_lines(&self, lines: &mut crate::debug_render::DebugLines, window_width: u32) {
        let width = (window_width as f32 / 1920.0 * 2.5).max(2.5);
        for &(a, b) in &self.objects.lines {
            lines.line(crate::debug_render::Layer::Depth, a, b, 0xff00_0000, width);
        }
    }
}

#[cfg(test)]
mod objects_frame_tests {
    use super::*;

    #[test]
    fn fishing_line_runs_from_hook_to_hand() {
        let o = [1.5, -2.0, 3.0];
        let pts = fishing_line(o);
        assert_eq!(pts[0], [0.0, 0.25, 0.0]);
        // End: y·(1 + 1)/2 + 0.25 = hand − bobber.
        assert_eq!(pts[16], [1.5, -1.75, 3.0]);
        // Parabola: in the middle, y·(0.25 + 0.5)/2 + 0.25.
        assert!((pts[8][1] - (-2.0 * 0.375 + 0.25)).abs() < 1e-6);
    }

    #[test]
    fn guardian_eye_turns_toward_target() {
        // Guardian looking south (+z): target to the east → x = 2, to the west → x = −2; target
        // above the eyes → y = 0, otherwise y = 1; target straight ahead → x = 0.
        let o = [0.0, 64.0, 0.0];
        assert_eq!(guardian_eye(o, [5.0, 64.0, 0.0], 0.0, 0.0), [2.0, 1.0]);
        assert_eq!(guardian_eye(o, [-5.0, 70.0, 0.0], 0.0, 0.0), [-2.0, 0.0]);
        let ahead = guardian_eye(o, [0.0, 60.0, 5.0], 0.0, 0.0);
        assert!(ahead[0].abs() < 1.0e-6 && ahead[1] == 1.0);
        // At 45°: root of cos 45° × 2.
        let d = guardian_eye(o, [5.0, 64.0, 5.0], 0.0, 0.0);
        assert!((d[0] - (0.5f32.sqrt()).sqrt() * 2.0).abs() < 1.0e-5, "{d:?}");
    }

    #[test]
    fn guardian_beam_color_follows_attack_scale() {
        assert_eq!(guardian_beam_color(0.0), 64 | 32 << 8 | 128 << 16 | 0xff00_0000);
        assert_eq!(guardian_beam_color(1.0), 255 | 223 << 8 | 64 << 16 | 0xff00_0000);
        // 0.5² × 191 = 47.75 → 47 (truncation).
        assert_eq!(guardian_beam_color(0.5), 111 | 79 << 8 | 112 << 16 | 0xff00_0000);
    }

    #[test]
    fn guardian_beam_spans_eye_to_target_plus_one() {
        let mut out = Vec::new();
        guardian_beam_quads(Mat4::IDENTITY, Vec3::new(0.0, 0.0, 4.0), 0.0, 0.0, [0.0; 4], &mut out);
        assert_eq!(out.len(), 18);
        // The top of the planes is at length + 1 along the direction (here +z).
        let far = out.iter().map(|v| v.pos[2]).fold(f32::MIN, f32::max);
        assert!((far - 5.0).abs() < 1e-4, "{far}");
        // Texture: v from −1 to −1 + 5 × 2.5.
        assert!(out.iter().any(|v| (v.uv[1] - 11.5).abs() < 1e-5));
    }

    #[test]
    fn text_opacity_and_background_interpolate_like_the_game() {
        let t = DisplayText {
            text: None,
            line_width: 200,
            opacity: (-1, 64),
            background: (0x4000_0000, 0x80ff_0000),
            flags: 0,
        };
        // −1 + floor(0.5 × 65) = 31.
        assert_eq!(t.opacity_at(0.5), 31);
        assert_eq!(t.opacity_at(1.0), 64);
        // Channel by channel: a 0x40→0x80 = 0x60, r 0→0xff = 0x7f.
        assert_eq!(t.background_at(0.5), 0x607f_0000);
    }
}

#[cfg(test)]
mod display_tests {
    use super::*;
    use crate::entities::Entities;
    use rc_client::EntityEvent;
    use rc_proto_v777::entity::{AddEntity, Meta};

    /// A block display spawned (id 7), with no data received.
    fn display() -> Entities {
        let mut all = Entities { type_names: vec!["minecraft:block_display".into()], ..Default::default() };
        let add = AddEntity {
            id: 7,
            uuid: 1,
            kind: 0,
            pos: [0.5, 64.0, 0.5],
            velocity: [0.0; 3],
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: 0,
        };
        all.apply(EntityEvent::Add(Box::new(add)));
        all
    }

    fn data(all: &mut Entities, mut values: Vec<(u8, Meta)>) {
        values.push((23, Meta::BlockState(Some(1))));
        all.apply(EntityEvent::Data { id: 7, values });
    }

    fn tick(all: &mut Entities) {
        all.tick();
    }

    /// Text display: opacity and background frozen at the tick, interpolated from the last render
    /// when a duration is running; text absent while nothing is received.
    #[test]
    fn text_state_freezes_then_interpolates() {
        let mut all = Entities { type_names: vec!["minecraft:text_display".into()], ..Default::default() };
        let add = AddEntity {
            id: 7,
            uuid: 1,
            kind: 0,
            pos: [0.5, 64.0, 0.5],
            velocity: [0.0; 3],
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: 0,
        };
        all.apply(EntityEvent::Add(Box::new(add)));
        all.apply(EntityEvent::Data { id: 7, values: vec![(26, Meta::Byte(-1)), (27, Meta::Byte(1 | 8))] });
        tick(&mut all);
        let text = |all: &Entities| match &all.map[&7].display.as_deref().unwrap().render.as_ref().unwrap().sub {
            DisplaySub::Text(t) => t.clone(),
            _ => panic!("no text"),
        };
        let t = text(&all);
        assert_eq!((t.opacity_at(0.0), t.background_at(0.0), t.line_width, t.flags), (-1, 0x4000_0000, 200, 9));
        // Duration 10 ticks, then new opacity and new background: start = rendered state (p = 0.5).
        all.apply(EntityEvent::Data { id: 7, values: vec![(9, Meta::Int(10)), (8, Meta::Int(0))] });
        tick(&mut all);
        all.map[&7].display.as_deref().unwrap().last.set(0.5);
        all.apply(EntityEvent::Data { id: 7, values: vec![(26, Meta::Byte(64)), (25, Meta::Int(0x0000_00ff))] });
        tick(&mut all);
        let t = text(&all);
        assert_eq!(t.opacity_at(0.0), -1);
        assert_eq!(t.opacity_at(1.0), 64);
        assert_eq!(t.background_at(0.0), 0x4000_0000);
        assert_eq!(t.background_at(1.0), 0x0000_00ff);
    }

    /// Cushion: the model's slab (16 × 4 × 16 px, placed upside down) occupies y ∈ [0, 0.25].
    #[test]
    fn cushion_box_rests_on_its_block() {
        let m = Mat4::from_rotation_y(deg(180.0))
            * Mat4::from_rotation_x(PI)
            * Mat4::from_translation(Vec3::new(0.0, -0.25, 0.0));
        // Corners of the slab after the part offset (23, 4, −7): x, z ∈ [−8, 8], y ∈ [0, 4] px.
        let ys: Vec<f32> =
            [0.0f32, 4.0].iter().map(|&y| m.transform_point3(Vec3::new(-8.0 / 16.0, y / 16.0, 8.0 / 16.0)).y).collect();
        assert!((ys[0] - 0.25).abs() < 1e-6 && ys[1].abs() < 1e-6, "{ys:?}");
    }

    /// Translation x displayed at render (`pt`), as the engine computes it (stored progress).
    fn shown_x(all: &Entities, pt: f32) -> Option<f32> {
        let e = &all.map[&7];
        let ds = e.display.as_deref()?;
        let rs = ds.render.as_ref()?;
        let p = ds.progress(e.age, pt);
        ds.last.set(p);
        Some(rs.pose(p).matrix().w_axis.x)
    }

    #[test]
    fn cull_box_only_with_nonzero_width_and_height() {
        let mut e = display();
        assert_eq!(display_cull_box(&e.map[&7]), None);
        data(&mut e, vec![(20, Meta::Float(3.0))]);
        assert_eq!(display_cull_box(&e.map[&7]), None, "zero height: never culled");
        data(&mut e, vec![(20, Meta::Float(3.0)), (21, Meta::Float(2.0))]);
        assert_eq!(display_cull_box(&e.map[&7]), Some((3.0, 2.0)));
    }

    #[test]
    fn nothing_before_first_tick_then_fresh_state() {
        let mut e = display();
        data(&mut e, vec![(11, Meta::Vector3([2.0, 0.0, 0.0]))]);
        assert_eq!(shown_x(&e, 0.5), None);
        tick(&mut e);
        assert_eq!(shown_x(&e, 0.5), Some(2.0));
    }

    #[test]
    fn transformation_interpolates_over_duration() {
        let mut e = display();
        data(&mut e, vec![(11, Meta::Vector3([0.0; 3]))]);
        tick(&mut e);
        // New pose over 10 ticks, immediate start.
        data(&mut e, vec![(8, Meta::Int(0)), (9, Meta::Int(10)), (11, Meta::Vector3([10.0, 0.0, 0.0]))]);
        assert_eq!(shown_x(&e, 0.0), Some(0.0), "nothing changes before the tick");
        tick(&mut e);
        // Duration applied at the same tick as the state: the interpolation starts from the displayed pose.
        let x0 = shown_x(&e, 0.5).unwrap();
        assert!((x0 - 0.5).abs() < 1e-5, "{x0}");
        for _ in 0..4 {
            tick(&mut e);
        }
        let x = shown_x(&e, 0.0).unwrap();
        assert!((x - 4.0).abs() < 1e-5, "{x}");
        for _ in 0..10 {
            tick(&mut e);
        }
        assert_eq!(shown_x(&e, 0.3), Some(10.0));
    }

    #[test]
    fn interrupted_interpolation_restarts_from_last_rendered_pose() {
        let mut e = display();
        data(&mut e, vec![(9, Meta::Int(10)), (8, Meta::Int(0)), (11, Meta::Vector3([0.0; 3]))]);
        tick(&mut e);
        data(&mut e, vec![(8, Meta::Int(0)), (11, Meta::Vector3([10.0, 0.0, 0.0]))]);
        tick(&mut e);
        for _ in 0..4 {
            tick(&mut e);
        }
        // Last render at 4.5 ticks out of 10: start of the next = 4.5.
        let x = shown_x(&e, 0.5).unwrap();
        assert!((x - 4.5).abs() < 1e-5, "{x}");
        data(&mut e, vec![(8, Meta::Int(0)), (11, Meta::Vector3([-10.0, 0.0, 0.0]))]);
        tick(&mut e);
        let x = shown_x(&e, 0.0).unwrap();
        assert!((x - 4.5).abs() < 1e-5, "{x}");
        let x = shown_x(&e, 1.0).unwrap();
        assert!((x - (4.5 + (-10.0 - 4.5) * 0.1)).abs() < 1e-4, "{x}");
    }

    #[test]
    fn delayed_start_holds_previous_pose() {
        let mut e = display();
        data(&mut e, vec![(11, Meta::Vector3([0.0; 3]))]);
        tick(&mut e);
        data(&mut e, vec![(8, Meta::Int(3)), (9, Meta::Int(2)), (11, Meta::Vector3([4.0, 0.0, 0.0]))]);
        tick(&mut e);
        assert_eq!(shown_x(&e, 0.9), Some(0.0));
        for _ in 0..3 {
            tick(&mut e);
        }
        let x = shown_x(&e, 0.5).unwrap();
        assert!((x - 1.0).abs() < 1e-5, "{x}");
    }

    #[test]
    fn slerp_matches_reference_values() {
        // Quarter turn around y, halfway: 45°.
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let q = slerp([0.0, 0.0, 0.0, 1.0], [0.0, h, 0.0, h], 0.5);
        let a = std::f32::consts::FRAC_PI_8;
        for (got, want) in q.iter().zip([0.0, a.sin(), 0.0, a.cos()]) {
            assert!((got - want).abs() < 1e-6, "{q:?}");
        }
        // Shortest path: target of opposite sign (same rotation), same result.
        let q = slerp([0.0, 0.0, 0.0, 1.0], [0.0, -h, 0.0, -h], 0.5);
        assert!((q[1] - a.sin()).abs() < 1e-6 && (q[3] - a.cos()).abs() < 1e-6, "{q:?}");
        // Almost equal: non-normalised linear blend.
        let q = slerp([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0, 1.0], 0.25);
        assert_eq!(q, [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn non_unit_quaternion_scales_like_the_game() {
        // [0, 0, 0, 2]: identity rotation, scale 4 (squared norm).
        let m = quat_matrix([0.0, 0.0, 0.0, 2.0]);
        assert_eq!(m, Mat4::from_scale(Vec3::splat(4.0)));
        let q = glam::Quat::from_rotation_z(0.7);
        let m = quat_matrix(q.to_array());
        assert!(m.abs_diff_eq(Mat4::from_quat(q), 1e-6));
    }

    #[test]
    fn shadow_interpolates_with_the_transformation() {
        let mut e = display();
        data(&mut e, vec![(18, Meta::Float(0.0))]);
        tick(&mut e);
        assert_eq!(display_shadow(&e.map[&7], 0.0), Some((0.0, 1.0)));
        data(&mut e, vec![(8, Meta::Int(0)), (9, Meta::Int(4)), (18, Meta::Float(2.0))]);
        tick(&mut e);
        let _ = shown_x(&e, 0.0);
        assert_eq!(display_shadow(&e.map[&7], 2.0), Some((1.0, 1.0)));
    }
}
