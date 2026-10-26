//! Blockstate files and construction ("baking") of each state's faces.
//! Rules: `docs/specs/models.md`.

use hashbrown::HashMap;

use rayon::prelude::*;
use serde_json::Value;

use rc_data::VersionData;

use crate::geom::{self, Dir, M3, V3};
use crate::model::{self, Model, Models};
use crate::source::{Assets, split_id};
use crate::texture::{self, Image};

/// Render layer of a face, deduced from the transparency of its texture area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Layer {
    Solid,
    Cutout,
    Translucent,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BakedQuad {
    /// Corners as a fraction of a block, in the official quad order.
    pub pos: [V3; 4],
    /// Texture coordinates as a fraction (0..1) of the texture.
    pub uv: [[f32; 2]; 4],
    pub texture: u32,
    pub tint: i32,
    pub cull: Option<Dir>,
    /// Direction of the normal after rotation (directional shading).
    pub dir: Dir,
    /// Direction that picks the shading: the one forced by the element, otherwise `dir`.
    pub shade_dir: Dir,
    pub layer: Layer,
    /// The model uses smooth lighting and ambient occlusion.
    pub ao: bool,
    /// Emission of the element (0–15).
    pub emission: u8,
}

#[derive(Debug, Clone, Copy)]
struct VariantKey {
    model: u32,
    x: i32,
    y: i32,
    z: i32,
    uvlock: bool,
}

/// A weighted list of variants; each option points to a range of `quads`.
#[derive(Debug, Clone, Default)]
pub struct Part {
    pub total_weight: u32,
    /// (weight, start, count) in `BlockModels::quads`.
    pub options: Vec<(u32, u32, u32)>,
}

#[derive(Debug, Clone, Default)]
pub struct StateModel {
    pub parts: Vec<Part>,
    /// Defined by cases (`multipart`): the draw first draws a long, then reseeds
    /// the generator with that long before each part.
    pub multipart: bool,
}

/// Fluid textures (indices into `BlockModels::textures`) and render layer of each,
/// deduced from the transparency of its textures (model rule).
#[derive(Debug, Clone, Copy)]
pub struct FluidTextures {
    pub water_still: u32,
    pub water_flow: u32,
    pub water_overlay: u32,
    pub lava_still: u32,
    pub lava_flow: u32,
    pub water_layer: Layer,
    pub lava_layer: Layer,
}

const FLUID_TEXTURES: [&str; 5] = [
    "minecraft:block/water_still",
    "minecraft:block/water_flow",
    "minecraft:block/water_overlay",
    "minecraft:block/lava_still",
    "minecraft:block/lava_flow",
];

pub struct BlockModels {
    pub fluids: FluidTextures,
    pub quads: Vec<BakedQuad>,
    pub states: Vec<StateModel>,
    /// Texture layers: one per texture (`texture_names`), then the animation frames;
    /// each with its mip levels (0 = `texture_size`).
    pub textures: Vec<Vec<Image>>,
    pub texture_names: Vec<String>,
    pub animations: Vec<TextureAnimation>,
    /// Common texture size (side in pixels).
    pub texture_size: u32,
    /// Size of the second array (textures larger than `texture_size`), 0 if there is none.
    pub big_texture_size: u32,
    /// Summary: states without a blockstate file, missing models, missing textures.
    pub missing: Missing,
    /// "particle" texture of each state (break particles, view inside a block): that of the
    /// first model of its chosen definition, like the game; index into `particle_names`.
    pub state_particles: Vec<u32>,
    /// Normalised identifiers (`namespace:path`); `model::MISSING_TEXTURE` if unresolved.
    pub particle_names: Vec<String>,
}

impl BlockModels {
    /// "particle" texture of the state (normalised identifier).
    pub fn particle(&self, state: u32) -> Option<&str> {
        let i = *self.state_particles.get(state as usize)?;
        self.particle_names.get(i as usize).map(String::as_str)
    }
}

/// Animation of a texture: the `texture` layer shows the layers of the steps in turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureAnimation {
    pub texture: u32,
    /// (layer, duration in ticks) in playback order.
    pub steps: Vec<(u32, u32)>,
    pub interpolate: bool,
}

/// Playback of an animation, advanced one client tick at a time (20 per second).
#[derive(Debug, Clone, Copy, Default)]
pub struct AnimationClock {
    step: usize,
    sub: u32,
}

impl AnimationClock {
    pub fn tick(&mut self, a: &TextureAnimation) {
        self.sub += 1;
        if self.sub >= a.steps[self.step].1 {
            self.step = (self.step + 1) % a.steps.len();
            self.sub = 0;
        }
    }

    /// (layer shown, next layer, share of the next one in thousandths).
    pub fn frame(&self, a: &TextureAnimation) -> (u32, u32, u32) {
        let (layer, time) = a.steps[self.step];
        if a.interpolate {
            let next = a.steps[(self.step + 1) % a.steps.len()].0;
            (layer, next, ((self.sub as f32 / time as f32) * 1000.0) as u32)
        } else {
            (layer, layer, 0)
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Missing {
    pub blockstates: usize,
    pub models: Vec<String>,
    pub textures: Vec<String>,
}

// Resource cache (`cache.rs`).
crate::codec_enum!(Layer { Solid, Cutout, Translucent });
crate::codec_struct!(BakedQuad { pos, uv, texture, tint, cull, dir, shade_dir, layer, ao, emission });
crate::codec_struct!(Part { total_weight, options });
crate::codec_struct!(StateModel { parts, multipart });
crate::codec_struct!(FluidTextures {
    water_still,
    water_flow,
    water_overlay,
    lava_still,
    lava_flow,
    water_layer,
    lava_layer
});
crate::codec_struct!(TextureAnimation { texture, steps, interpolate });
crate::codec_struct!(Missing { blockstates, models, textures });
crate::codec_struct!(BlockModels {
    fluids,
    quads,
    states,
    textures,
    texture_names,
    animations,
    texture_size,
    big_texture_size,
    missing,
    state_particles,
    particle_names
});

/// The game's 48-bit linear congruential generator (draw seed of the variants).
#[derive(Debug, Clone, Copy)]
pub struct Lcg(u64);

impl Lcg {
    const MUL: u64 = 0x5_DEEC_E66D;
    const MASK: u64 = (1 << 48) - 1;

    pub fn new(seed: i64) -> Self {
        Lcg((seed as u64 ^ Self::MUL) & Self::MASK)
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.0 = (self.0.wrapping_mul(Self::MUL).wrapping_add(11)) & Self::MASK;
        (self.0 >> (48 - bits)) as i32
    }

    pub fn next_double(&mut self) -> f64 {
        let hi = i64::from(self.next(26));
        let lo = i64::from(self.next(27));
        ((hi << 27) + lo) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    pub fn next_long(&mut self) -> i64 {
        let hi = i64::from(self.next(32));
        let lo = i64::from(self.next(32));
        (hi << 32).wrapping_add(lo)
    }

    pub fn next_int(&mut self, bound: u32) -> u32 {
        let bound = bound as i32;
        if bound & (bound - 1) == 0 {
            return ((i64::from(bound) * i64::from(self.next(31))) >> 31) as u32;
        }
        loop {
            let sample = self.next(31);
            let modulo = sample % bound;
            if sample.wrapping_sub(modulo).wrapping_add(bound - 1) >= 0 {
                return modulo as u32;
            }
        }
    }
}

/// Official position seed. The product in x is done in 32-bit integers (it overflows from
/// |x| > 686, like in the game) before widening; the one in z is in 64 bits.
pub fn position_seed(x: i32, y: i32, z: i32) -> i64 {
    let mut s = i64::from(x.wrapping_mul(3_129_871)) ^ (i64::from(z).wrapping_mul(116_129_781)) ^ i64::from(y);
    s = s.wrapping_mul(s).wrapping_mul(42_317_861).wrapping_add(s.wrapping_mul(11));
    s >> 16
}

impl BlockModels {
    /// Faces of state `state` at the given position (variant draw identical to the game).
    pub fn select(&self, state: u32, seed_offset: [i32; 3], pos: [i32; 3], out: &mut Vec<u32>) {
        out.clear();
        let Some(m) = self.states.get(state as usize) else { return };
        m.select(|| position_seed(pos[0] + seed_offset[0], pos[1] + seed_offset[1], pos[2] + seed_offset[2]), out);
    }
}

impl StateModel {
    /// Appends the drawn face ranges to `out`; `seed` gives the position seed, computed
    /// only if there is a draw. By variants: the single list is drawn directly with
    /// that seed. By cases: a long is drawn from that seed, then the generator is reseeded
    /// with that long before each part (two parts with equal weights therefore draw the same rank).
    pub fn select(&self, seed: impl Fn() -> i64, out: &mut Vec<u32>) {
        let mut part_seed = None;
        for part in &self.parts {
            let (start, count) = if part.options.len() == 1 {
                (part.options[0].1, part.options[0].2)
            } else {
                let s = *part_seed
                    .get_or_insert_with(|| if self.multipart { Lcg::new(seed()).next_long() } else { seed() });
                let mut pick = Lcg::new(s).next_int(part.total_weight);
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
            out.extend(start..start + count);
        }
    }
}

// ---------- Blockstate files ----------

fn parse_variant_list(
    v: &Value,
    model_ids: &mut HashMap<String, u32>,
    names: &mut Vec<String>,
) -> Vec<(u32, VariantKey)> {
    let one = |o: &Value, model_ids: &mut HashMap<String, u32>, names: &mut Vec<String>| -> Option<(u32, VariantKey)> {
        let model = o.get("model")?.as_str()?;
        let (ns, path) = split_id(model);
        let key = format!("{ns}:{path}");
        let next = names.len() as u32;
        let id = *model_ids.entry(key.clone()).or_insert_with(|| {
            names.push(key);
            next
        });
        let r = |k: &str| o.get(k).and_then(Value::as_i64).unwrap_or(0).rem_euclid(360) as i32;
        Some((
            o.get("weight").and_then(Value::as_u64).unwrap_or(1).max(1) as u32,
            VariantKey {
                model: id,
                x: r("x"),
                y: r("y"),
                z: r("z"),
                uvlock: o.get("uvlock").and_then(Value::as_bool).unwrap_or(false),
            },
        ))
    };
    match v {
        Value::Array(a) => a.iter().filter_map(|o| one(o, model_ids, names)).collect(),
        o => one(o, model_ids, names).into_iter().collect(),
    }
}

/// Parts of a state: for each, its weighted variants (weight, variant).
type StateParts = Vec<Vec<(u32, VariantKey)>>;

/// Parts of a state and whether it is defined by cases, from the stacked blockstate files (top
/// pack first): the first one that covers the state wins (matching variant; `multipart`
/// cases cover every state). `None`: none covers it. The last element is the
/// model that gives the particle, like the game: first entry of the variant, or of the first
/// case (matched or not); `MISSING_MODEL` if the list is empty.
fn state_variants(
    defs: &[Value],
    props: &[(&str, &str)],
    model_ids: &mut HashMap<String, u32>,
    names: &mut Vec<String>,
) -> Option<(StateParts, bool, u32)> {
    let first = |l: &[(u32, VariantKey)]| l.first().map_or(MISSING_MODEL, |e| e.1.model);
    defs.iter().find_map(|def| {
        let variant = def
            .get("variants")
            .and_then(Value::as_object)
            .and_then(|vars| vars.iter().find(|(k, _)| matches_key(k, props)));
        if let Some((_, v)) = variant {
            let list = parse_variant_list(v, model_ids, names);
            let particle = first(&list);
            return Some((vec![list], false, particle));
        }
        let mp = def.get("multipart").and_then(Value::as_array)?;
        let particle = mp
            .first()
            .and_then(|case| case.get("apply"))
            .map_or(MISSING_MODEL, |apply| first(&parse_variant_list(apply, model_ids, names)));
        let parts = mp
            .iter()
            .filter(|case| case.get("when").is_none_or(|w| matches_when(w, props)))
            .filter_map(|case| case.get("apply"))
            .map(|apply| parse_variant_list(apply, model_ids, names))
            .collect();
        Some((parts, true, particle))
    })
}

/// True if `cond` (`when` object) is satisfied by the state's properties.
fn matches_when(cond: &Value, props: &[(&str, &str)]) -> bool {
    let Some(obj) = cond.as_object() else { return true };
    if let Some(any) = obj.get("OR").and_then(Value::as_array) {
        return any.iter().any(|c| matches_when(c, props));
    }
    if let Some(all) = obj.get("AND").and_then(Value::as_array) {
        return all.iter().all(|c| matches_when(c, props));
    }
    obj.iter().all(|(k, v)| {
        let want = match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        // Terms separated by `|`, each negated by its own `!`: "!a|b" = (≠ a) or (= b).
        let have = props.iter().find(|(n, _)| n == k).map(|(_, v)| *v);
        have.is_some_and(|h| {
            want.split('|').any(|t| match t.strip_prefix('!') {
                Some(v) => v != h,
                None => t == h,
            })
        })
    })
}

/// Satisfied variant key: each `prop=value` of the key matches the state.
fn matches_key(key: &str, props: &[(&str, &str)]) -> bool {
    key.is_empty()
        || key.split(',').all(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            props.iter().any(|(n, val)| *n == k && *val == v)
        })
}

// ---------- Baking ----------

/// Corners of a face (official order), as indices 0 = min, 1 = max for x, y, z.
pub(crate) const CORNERS: [[[u8; 3]; 4]; 6] = [
    [[0, 0, 1], [0, 0, 0], [1, 0, 0], [1, 0, 1]],
    [[0, 1, 0], [0, 1, 1], [1, 1, 1], [1, 1, 0]],
    [[1, 1, 0], [1, 0, 0], [0, 0, 0], [0, 1, 0]],
    [[0, 1, 1], [0, 0, 1], [1, 0, 1], [1, 1, 1]],
    [[0, 1, 0], [0, 0, 0], [0, 0, 1], [0, 1, 1]],
    [[1, 1, 1], [1, 0, 1], [1, 0, 0], [1, 1, 0]],
];

pub(crate) fn default_uv(f: V3, t: V3, d: Dir) -> [f32; 4] {
    match d {
        Dir::Down => [f[0], 16.0 - t[2], t[0], 16.0 - f[2]],
        Dir::Up => [f[0], f[2], t[0], t[2]],
        Dir::North => [16.0 - t[0], 16.0 - t[1], 16.0 - f[0], 16.0 - f[1]],
        Dir::South => [f[0], 16.0 - t[1], t[0], 16.0 - f[1]],
        Dir::West => [f[2], 16.0 - t[1], t[2], 16.0 - f[1]],
        Dir::East => [16.0 - t[2], 16.0 - t[1], 16.0 - f[2], 16.0 - f[1]],
    }
}

/// Local → global frame of each face for texture locking.
fn local_to_global(d: Dir) -> M3 {
    match d {
        Dir::South => geom::IDENTITY,
        Dir::East => geom::rotation_90(geom::Y, 90),
        Dir::West => geom::rotation_90(geom::Y, -90),
        Dir::North => geom::rotation_90(geom::Y, 180),
        Dir::Up => geom::rotation_90(geom::X, -90),
        Dir::Down => geom::rotation_90(geom::X, 90),
    }
}

/// Inverse of the face transform (pure rotations: inverse = transpose).
fn uvlock_matrix(rot: &M3, face: Dir) -> M3 {
    let action = geom::mul(rot, &local_to_global(face));
    let new_side = Dir::nearest(geom::mul_v(&action, [0.0, 0.0, 1.0])).unwrap_or(face);
    let transform = geom::mul(&geom::transpose(&local_to_global(new_side)), &action);
    geom::transpose(&transform)
}

struct TextureTable {
    index: HashMap<String, u32>,
    names: Vec<String>,
}

impl TextureTable {
    fn id(&mut self, name: &str) -> u32 {
        let (ns, path) = split_id(name);
        let key = format!("{ns}:{path}");
        if let Some(&i) = self.index.get(&key) {
            return i;
        }
        let i = self.names.len() as u32;
        self.index.insert(key.clone(), i);
        self.names.push(key);
        i
    }
}

/// Faces of a variant (flattened model + rotation), in elements × directions order.
fn bake_variant(
    model: &Model,
    key: &VariantKey,
    textures: &mut TextureTable,
    images: &HashMap<String, Image>,
    animated: &HashMap<String, (Vec<Image>, texture::Animation)>,
    missing_tex: &mut Vec<String>,
) -> Vec<BakedQuad> {
    let rot = geom::variant_rotation(key.x, key.y, key.z);
    let rotated = rot != geom::IDENTITY;
    let mut out = Vec::new();
    for e in model.elements.iter() {
        for d in Dir::ALL {
            let Some(face) = &e.faces[d as usize] else { continue };
            let tex = model.resolve(&face.texture);
            let (tex_name, force) = match &tex {
                Some(t) => (t.value.clone(), t.force_translucent),
                None => {
                    missing_tex.push(face.texture.clone());
                    (model::MISSING_TEXTURE.to_owned(), false)
                }
            };
            let tex_name = {
                let (ns, path) = split_id(&tex_name);
                format!("{ns}:{path}")
            };
            let uvs = face.uv.unwrap_or_else(|| default_uv(e.from, e.to, d));
            let layer = if force {
                Layer::Translucent
            } else {
                let t = images.get(&tex_name).map(|img| {
                    texture::sprite_transparency(
                        img,
                        animated.get(&tex_name).map(|(f, a)| (f.as_slice(), a)),
                        uvs[0].min(uvs[2]) / 16.0,
                        uvs[1].min(uvs[3]) / 16.0,
                        uvs[0].max(uvs[2]) / 16.0,
                        uvs[1].max(uvs[3]) / 16.0,
                    )
                });
                match t {
                    Some(t) if t.translucent => Layer::Translucent,
                    Some(t) if t.transparent => Layer::Cutout,
                    _ => Layer::Solid,
                }
            };
            let uv_m = (key.uvlock && rotated).then(|| uvlock_matrix(&rot, d));
            let mut pos = [[0.0f32; 3]; 4];
            let mut uv = [[0.0f32; 2]; 4];
            for i in 0..4 {
                let c = CORNERS[d as usize][i];
                let mut p: V3 = [0, 1, 2].map(|a| if c[a] == 1 { e.to[a] } else { e.from[a] } / 16.0);
                if let Some(r) = &e.rotation {
                    let q = geom::mul_v(&r.matrix, [p[0] - r.origin[0], p[1] - r.origin[1], p[2] - r.origin[2]]);
                    p = [q[0] + r.origin[0], q[1] + r.origin[1], q[2] + r.origin[2]];
                }
                if rotated {
                    let q = geom::mul_v(&rot, [p[0] - 0.5, p[1] - 0.5, p[2] - 0.5]);
                    p = [q[0] + 0.5, q[1] + 0.5, q[2] + 0.5];
                }
                pos[i] = p;
                let j = (i + face.rotation as usize) % 4;
                let raw_u = if j == 0 || j == 1 { uvs[0] } else { uvs[2] } / 16.0;
                let raw_v = if j == 0 || j == 3 { uvs[1] } else { uvs[3] } / 16.0;
                uv[i] = match &uv_m {
                    None => [raw_u, raw_v],
                    Some(m) => {
                        let t = geom::mul_v(m, [raw_u - 0.5, raw_v - 0.5, 0.0]);
                        [t[0] + 0.5, t[1] + 0.5]
                    }
                };
            }
            // Normal after rotation (cross product of the first two edges).
            let a = [pos[1][0] - pos[0][0], pos[1][1] - pos[0][1], pos[1][2] - pos[0][2]];
            let b = [pos[2][0] - pos[1][0], pos[2][1] - pos[1][1], pos[2][2] - pos[1][2]];
            let n = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
            let near = Dir::nearest(n);
            // Element without its own rotation: vertices put back in the order of the destination face (the
            // diagonal of the two triangles, hence the shading gradient, depends on it).
            if let (None, Some(d)) = (&e.rotation, near) {
                canonical_winding(&mut pos, &mut uv, d);
            }
            let dir = near.unwrap_or(Dir::Up);
            let cull =
                face.cull.map(|c| if rotated { Dir::nearest(geom::mul_v(&rot, c.normal())).unwrap_or(c) } else { c });
            out.push(BakedQuad {
                pos,
                uv,
                texture: textures.id(&tex_name),
                tint: face.tint,
                cull,
                dir,
                shade_dir: e.shade_override.unwrap_or(dir),
                layer,
                ao: model.ambient_occlusion,
                emission: e.light_emission,
            });
        }
    }
    out
}

/// Reorders the 4 vertices of a face (and their UVs) into the order of `CORNERS` for `d`, from
/// the bounding box of the vertices: each expected corner is looked up among the remaining vertices
/// and swapped into place. Corner not found (non-aligned face): order left as is.
fn canonical_winding(pos: &mut [[f32; 3]; 4], uv: &mut [[f32; 2]; 4], d: Dir) {
    let mut lo = [999.0f32; 3];
    let mut hi = [-999.0f32; 3];
    for p in pos.iter() {
        for a in 0..3 {
            lo[a] = lo[a].min(p[a]);
            hi[a] = hi[a].max(p[a]);
        }
    }
    let (mut p, mut t) = (*pos, *uv);
    for (i, c) in CORNERS[d as usize].iter().enumerate() {
        let want: [f32; 3] = [0, 1, 2].map(|a| if c[a] == 1 { hi[a] } else { lo[a] });
        let Some(j) = (i..4).find(|&j| p[j] == want) else { return };
        p.swap(i, j);
        t.swap(i, j);
    }
    (*pos, *uv) = (p, t);
}

/// Combination baked only once: (model, x, y, z, texture lock).
type BakeKey = (u32, i32, i32, i32, bool);

/// Model of the states that no blockstate file covers (missing cube).
const MISSING_MODEL: u32 = u32::MAX;

/// Loads the states of all blocks and bakes their faces. Textures are loaded in
/// parallel on the pool; missing resources give a neutral fallback.
pub fn build(assets: &Assets, data: &VersionData) -> BlockModels {
    let models = Models::new(assets);
    let mut missing = Missing::default();
    let mut model_ids: HashMap<String, u32> = HashMap::new();
    let mut model_names: Vec<String> = Vec::new();

    // 1. Blockstate files of each block in each pack (top pack first), read in parallel;
    // an unreadable file only removes the share of its pack.
    let defs: Vec<Vec<Value>> = data
        .blocks
        .par_iter()
        .map(|b| {
            let (ns, path) = split_id(&b.name);
            assets
                .read_all(&format!("assets/{ns}/blockstates/{path}.json"))
                .iter()
                .filter_map(|bytes| serde_json::from_slice(bytes).ok())
                .collect()
        })
        .collect();

    // 2. Variants of each state: packs merged, the highest one that covers the state wins (a
    // variant that matches it, or `multipart` cases, which cover every state). State
    // covered by none: missing cube.
    let mut state_parts: Vec<StateParts> =vec![Vec::new(); data.states.len()];
    let mut state_multipart = vec![false; data.states.len()];
    let mut particle_model = vec![MISSING_MODEL; data.states.len()];
    for (b, defs) in data.blocks.iter().zip(&defs) {
        for sid in b.first_state..b.first_state + b.state_count {
            let props: Vec<(&str, &str)> = data.state_properties(sid).collect();
            match state_variants(defs, &props, &mut model_ids, &mut model_names) {
                Some((parts, multipart, particle)) => {
                    state_parts[sid as usize] = parts;
                    state_multipart[sid as usize] = multipart;
                    particle_model[sid as usize] = particle;
                }
                None => {
                    missing.blockstates += 1;
                    state_parts[sid as usize] =
                        vec![vec![(1, VariantKey { model: MISSING_MODEL, x: 0, y: 0, z: 0, uvlock: false })]];
                }
            }
        }
    }

    // 3. Flattened models and referenced textures.
    let flat: Vec<Option<Model>> = model_names.par_iter().map(|n| models.flatten(n)).collect();
    for (n, m) in model_names.iter().zip(&flat) {
        if m.is_none() {
            missing.models.push(n.clone());
        }
    }
    // Particle of each state: "particle" texture of its model (missing cube and unresolved
    // reference: missing texture, like the game).
    let mut particle_names: Vec<String> = Vec::new();
    let mut particle_index: HashMap<String, u32> = HashMap::new();
    let mut model_particle = |m: u32| -> u32 {
        let name = flat
            .get(m as usize)
            .and_then(Option::as_ref)
            .and_then(|m| m.resolve("particle"))
            .map_or_else(|| model::MISSING_TEXTURE.to_owned(), |t| {
                let (ns, path) = split_id(&t.value);
                format!("{ns}:{path}")
            });
        *particle_index.entry(name.clone()).or_insert_with(|| {
            particle_names.push(name);
            particle_names.len() as u32 - 1
        })
    };
    let mut by_model: HashMap<u32, u32> = HashMap::new();
    let state_particles: Vec<u32> =
        particle_model.iter().map(|&m| *by_model.entry(m).or_insert_with(|| model_particle(m))).collect();
    let mut tex_names: Vec<String> = flat
        .iter()
        .flatten()
        .flat_map(|m| {
            m.elements.iter().flat_map(|e| e.faces.iter().flatten().filter_map(|f| m.resolve(&f.texture))).map(|t| {
                let (ns, path) = split_id(&t.value);
                format!("{ns}:{path}")
            })
        })
        .collect();
    tex_names.extend(FLUID_TEXTURES.iter().map(|s| s.to_string()));
    tex_names.retain(|n| n != model::MISSING_TEXTURE);
    tex_names.sort_unstable();
    tex_names.dedup();
    let loaded: Vec<(String, Option<texture::Loaded>)> =
        tex_names.par_iter().map(|n| (n.clone(), texture::load_animated(assets, n))).collect();
    // Image shown at the start (first step of the animation), and animated frames separately.
    let mut images: HashMap<String, Image> = HashMap::new();
    let mut animated: HashMap<String, (Vec<Image>, texture::Animation)> = HashMap::new();
    // Mip levels of each frame of each texture (computed like the game at load time).
    let mut mips: HashMap<String, Vec<Vec<Image>>> = HashMap::new();
    for (n, l) in loaded {
        match l {
            Some(mut l) => {
                let first = l.animation.as_ref().map_or(0, |a| a.frames[0].0 as usize);
                images.insert(n.clone(), l.frames[first].clone());
                mips.insert(n.clone(), std::mem::take(&mut l.levels));
                if let Some(a) = l.animation.take() {
                    animated.insert(n, (l.frames, a));
                }
            }
            None => missing.textures.push(n),
        }
    }

    // 4. Baking, once per combination (model, rotation, lock); states → ranges.
    let mut textures = TextureTable { index: HashMap::new(), names: Vec::new() };
    textures.id(model::MISSING_TEXTURE);
    let mut quads: Vec<BakedQuad> = Vec::new();
    let mut baked: HashMap<BakeKey, (u32, u32)> = HashMap::new();
    let cube = Model::missing();
    let mut states = Vec::with_capacity(state_parts.len());
    for (parts, &multipart) in state_parts.iter().zip(&state_multipart) {
        let mut sm = StateModel { multipart, ..StateModel::default() };
        for list in parts {
            let mut part = Part::default();
            for &(weight, key) in list {
                let k = (key.model, key.x, key.y, key.z, key.uvlock);
                let range = *baked.entry(k).or_insert_with(|| {
                    let start = quads.len() as u32;
                    // Model not found or unreadable: missing cube, like the game.
                    let m = flat.get(key.model as usize).and_then(Option::as_ref).unwrap_or(&cube);
                    quads.extend(bake_variant(m, &key, &mut textures, &images, &animated, &mut missing.textures));
                    (start, quads.len() as u32 - start)
                });
                part.total_weight += weight;
                part.options.push((weight, range.0, range.1));
            }
            if !part.options.is_empty() {
                sm.parts.push(part);
            }
        }
        states.push(sm);
    }

    // Fluids: textures and layer (translucent if a semi-transparent pixel, otherwise cutout/opaque).
    let layer_of = |names: &[&str]| {
        let t = names.iter().filter_map(|n| images.get(*n).map(|i| (n, i))).fold(
            texture::Transparency::default(),
            |a, (n, i)| {
                let anim = animated.get(*n).map(|(f, an)| (f.as_slice(), an));
                let t = texture::sprite_transparency(i, anim, 0.0, 0.0, 1.0, 1.0);
                texture::Transparency {
                    transparent: a.transparent || t.transparent,
                    translucent: a.translucent || t.translucent,
                }
            },
        );
        if t.translucent {
            Layer::Translucent
        } else if t.transparent {
            Layer::Cutout
        } else {
            Layer::Solid
        }
    };
    let fluids = FluidTextures {
        water_still: textures.id(FLUID_TEXTURES[0]),
        water_flow: textures.id(FLUID_TEXTURES[1]),
        water_overlay: textures.id(FLUID_TEXTURES[2]),
        lava_still: textures.id(FLUID_TEXTURES[3]),
        lava_flow: textures.id(FLUID_TEXTURES[4]),
        water_layer: layer_of(&FLUID_TEXTURES[0..3]),
        lava_layer: layer_of(&FLUID_TEXTURES[3..5]),
    };

    // 5. Frames in index order. Main size: the largest up to 2 times the most
    // frequent (32 in the base game: flowing water and lava; the rest, at 16, is enlarged
    // by pixel duplication, same nearest-neighbour rendering, mipmaps included). Larger
    // textures (the straw bed, 64) go into a second array at their size, instead of making
    // every layer grow. Memory guard: at most 4 times the most frequent size.
    let mut sizes: HashMap<u32, usize> = HashMap::new();
    for img in images.values().chain(animated.values().flat_map(|(f, _)| f.iter())) {
        *sizes.entry(img.width).or_default() += 1;
    }
    let common = sizes.iter().max_by_key(|&(&s, &n)| (n, s)).map_or(16, |(&s, _)| s);
    let size = sizes.keys().copied().filter(|&s| s <= common * 2).max().unwrap_or(common);
    let big = sizes.keys().copied().filter(|&s| s > size && s <= common * 4).max().unwrap_or(0);
    let placed = |levels: Option<&Vec<Image>>| match levels {
        Some(l) if big > 0 && l[0].width > size => crate::mip::place(l, big),
        Some(l) => crate::mip::place(l, size),
        None => crate::mip::generate(Image::missing(size), size.ilog2(), crate::mip::Strategy::Mean, 0.0),
    };
    let mut tex_images: Vec<Vec<Image>> = textures
        .names
        .par_iter()
        .map(|n| {
            let first = animated.get(n).map_or(0, |(_, a)| a.frames[0].0 as usize);
            placed(mips.get(n).and_then(|f| f.get(first)))
        })
        .collect();
    // 6. Animations: each used frame takes a layer after the textures (the first
    // step reuses the texture's layer).
    let mut animations = Vec::new();
    for (id, name) in textures.names.iter().enumerate() {
        let Some((_, a)) = animated.get(name) else { continue };
        let mut layer_of: HashMap<u32, u32> = HashMap::from([(a.frames[0].0, id as u32)]);
        let steps = a
            .frames
            .iter()
            .map(|&(f, t)| {
                let layer = *layer_of.entry(f).or_insert_with(|| {
                    tex_images.push(placed(mips.get(name).and_then(|m| m.get(f as usize))));
                    tex_images.len() as u32 - 1
                });
                (layer, t)
            })
            .collect();
        animations.push(TextureAnimation { texture: id as u32, steps, interpolate: a.interpolate });
    }
    missing.textures.sort_unstable();
    missing.textures.dedup();
    BlockModels {
        fluids,
        quads,
        states,
        animations,
        textures: tex_images,
        texture_names: textures.names,
        texture_size: size,
        big_texture_size: big,
        missing,
        state_particles,
        particle_names,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_seed_and_lcg_reference_values() {
        // Reference values computed with the official formulas (64-bit Java arithmetic).
        assert_eq!(position_seed(0, 0, 0), 0);
        let s = position_seed(1, 2, 3);
        let mut r = Lcg::new(s);
        let a = r.next_int(4);
        assert!(a < 4);
        // Same seed → same sequence.
        let mut r2 = Lcg::new(s);
        assert_eq!(r2.next_int(4), a);
    }

    #[test]
    fn position_seed_overflows_like_java() {
        // Values obtained with Java arithmetic (product in x on 32 bits).
        assert_eq!(position_seed(1000, 64, -2000), 8_192_780_778_412);
        assert_eq!(position_seed(-700, 5, 300), -23_973_350_422_588);
        assert_eq!(position_seed(686, 0, 0), -36_903_954_281_238);
        assert_eq!(position_seed(687, 0, 0), -31_399_196_507_547);
        assert_eq!(position_seed(30_000_000, -64, -30_000_000), -36_386_240_128_100);
        assert_eq!(Lcg::new(position_seed(1000, 64, -2000)).next_long(), -1_441_060_552_199_276_918);
    }

    #[test]
    fn multipart_reseeds_each_part() {
        // Two parts with 3 and 5 options of weight 1: each reseeded with the same long, the game
        // draws rank 1 in both (Java reference), instead of a shared sequence.
        let part = |n: u32| Part { total_weight: n, options: (0..n).map(|i| (1, i * 10, 1)).collect() };
        let m = StateModel { parts: vec![part(3), part(5)], multipart: true };
        let mut out = Vec::new();
        m.select(|| position_seed(1000, 64, -2000), &mut out);
        assert_eq!(out, vec![10, 10]);
        // By variants: direct draw with the position seed.
        let v = StateModel { parts: vec![part(4)], multipart: false };
        let mut out2 = Vec::new();
        v.select(|| position_seed(1000, 64, -2000), &mut out2);
        let expect = Lcg::new(position_seed(1000, 64, -2000)).next_int(4) * 10;
        assert_eq!(out2, vec![expect]);
    }

    #[test]
    fn when_conditions() {
        let props = [("north", "true"), ("east", "false"), ("up", "low")];
        assert!(matches_when(&serde_json::json!({"north": "true"}), &props));
        assert!(!matches_when(&serde_json::json!({"east": "true"}), &props));
        assert!(matches_when(&serde_json::json!({"up": "low|tall"}), &props));
        assert!(matches_when(&serde_json::json!({"OR": [{"east": "true"}, {"north": "true"}]}), &props));
        assert!(matches_when(&serde_json::json!({"north": true}), &props));
        assert!(matches_key("east=false,north=true", &props));
        assert!(!matches_key("east=true", &props));
        assert!(matches_key("", &props));
    }

    /// "!a|b": each term carries its own negation, (≠ a) or (= b).
    #[test]
    fn when_negates_each_term() {
        let cond = serde_json::json!({"up": "!low|tall"});
        assert!(!matches_when(&cond, &[("up", "low")]));
        assert!(matches_when(&cond, &[("up", "tall")]));
        assert!(matches_when(&cond, &[("up", "none")]));
        // Term negated after the bar: "low|!tall" = everything but tall, low included.
        let cond = serde_json::json!({"up": "low|!tall"});
        assert!(matches_when(&cond, &[("up", "low")]));
        assert!(!matches_when(&cond, &[("up", "tall")]));
        assert!(matches_when(&cond, &[("up", "none")]));
    }

    /// Top face of a variant rotated by 90° in y: vertices put back in the order of the face
    /// (x-, z- corner first), UVs following their vertex; without rotation, order unchanged.
    #[test]
    fn rotated_variant_faces_are_rewound() {
        let rot = geom::variant_rotation(0, 90, 0);
        let mut pos = CORNERS[Dir::Up as usize].map(|c| {
            let p = [0, 1, 2].map(|a| f32::from(c[a]));
            let q = geom::mul_v(&rot, [p[0] - 0.5, p[1] - 0.5, p[2] - 0.5]);
            [q[0] + 0.5, q[1] + 0.5, q[2] + 0.5]
        });
        let mut uv = [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];
        let before = (pos, uv);
        canonical_winding(&mut pos, &mut uv, Dir::Up);
        let want = CORNERS[Dir::Up as usize].map(|c| [0, 1, 2].map(|a| f32::from(c[a])));
        assert_eq!(pos, want);
        assert_ne!(pos, before.0);
        // Each UV stays attached to its vertex.
        for i in 0..4 {
            let k = before.0.iter().position(|p| *p == pos[i]).unwrap();
            assert_eq!(uv[i], before.1[k]);
        }
        let (mut p2, mut t2) = (want, before.1);
        canonical_winding(&mut p2, &mut t2, Dir::Up);
        assert_eq!((p2, t2), (want, before.1));
    }

    /// Full bake of a rotated cube (y = 90, then x = 90): each face comes out in the canonical
    /// order of its destination direction.
    #[test]
    fn baked_rotated_cube_has_canonical_corners() {
        for (x, y) in [(0, 90), (90, 0), (90, 270)] {
            let key = VariantKey { model: 0, x, y, z: 0, uvlock: false };
            let mut tex = TextureTable { index: HashMap::new(), names: Vec::new() };
            let quads =
                bake_variant(&Model::missing(), &key, &mut tex, &HashMap::new(), &HashMap::new(), &mut Vec::new());
            assert_eq!(quads.len(), 6);
            for q in &quads {
                let want = CORNERS[q.dir as usize].map(|c| [0, 1, 2].map(|a| f32::from(c[a])));
                assert_eq!(q.pos, want, "x={x} y={y} face {:?}", q.dir);
            }
        }
    }

    #[test]
    fn blockstates_merge_per_state_top_pack_first() {
        let (mut ids, mut names) = (HashMap::new(), Vec::new());
        // Top pack: only facing=north; jar: all states; middle pack: cases.
        let top = serde_json::json!({"variants": {"facing=north": {"model": "p:top"}}});
        let jar = serde_json::json!({"variants": {"": {"model": "minecraft:block/bottom"}}});
        let mp = serde_json::json!({"multipart": [{"when": {"facing": "east"}, "apply": {"model": "m:east"}}]});
        let defs = [top.clone(), jar.clone()];
        let mut model_of = |defs: &[Value], facing: &str| {
            let (parts, multipart, _) = state_variants(defs, &[("facing", facing)], &mut ids, &mut names)?;
            Some((parts.iter().map(|p| names[p[0].1.model as usize].clone()).collect::<Vec<_>>(), multipart))
        };
        assert_eq!(model_of(&defs, "north"), Some((vec!["p:top".to_owned()], false)));
        assert_eq!(model_of(&defs, "south"), Some((vec!["minecraft:block/bottom".to_owned()], false)));
        // Cases cover every state, even without a matched case (nothing to draw).
        let defs = [top.clone(), mp, jar.clone()];
        assert_eq!(model_of(&defs, "south"), Some((vec![], true)));
        // No file covers the state: missing cube.
        assert_eq!(model_of(std::slice::from_ref(&top), "south"), None);
        // Particle: first entry of the chosen variant, or of the first case even if unmatched.
        let mut particle_of = |defs: &[Value], facing: &str| {
            let (_, _, m) = state_variants(defs, &[("facing", facing)], &mut ids, &mut names)?;
            Some(names[m as usize].clone())
        };
        let weighted = serde_json::json!({"variants": {"": [{"model": "a:one"}, {"model": "a:two"}]}});
        assert_eq!(particle_of(&[weighted], "north").as_deref(), Some("a:one"));
        let mp = serde_json::json!({"multipart": [
            {"when": {"facing": "east"}, "apply": [{"model": "m:first"}, {"model": "m:other"}]},
            {"apply": {"model": "m:second"}}
        ]});
        assert_eq!(particle_of(&[mp], "south").as_deref(), Some("m:first"));
        assert_eq!(particle_of(&[top, jar], "south").as_deref(), Some("minecraft:block/bottom"));
    }

    #[test]
    fn element_less_models_draw_nothing_and_broken_chains_end_on_the_missing_model() {
        use crate::source::testkit::{dir, stack};
        let m = "assets/minecraft/models/block/";
        let base = dir(
            "mdl-base",
            &[
                (&format!("{m}air.json"), br#"{"textures":{"particle":"minecraft:missingno"}}"#),
                (&format!("{m}orphan.json"), br#"{"parent":"minecraft:block/absent"}"#),
                (
                    &format!("{m}child.json"),
                    br##"{"parent":"minecraft:block/absnet","textures":{"all":"minecraft:block/stone"},
                    "elements":[{"from":[0,0,0],"to":[16,8,16],"faces":{"up":{"texture":"#all"}}}]}"##,
                ),
            ],
        );
        let assets = stack(&[], &base);
        let models = Models::new(&assets);
        // Particle only (air, water, chest): empty geometry, no missing cube.
        assert!(models.flatten("minecraft:block/air").is_some_and(|m| m.elements.is_empty()));
        // Model not found: missing cube (drawn by the caller).
        assert!(models.flatten("minecraft:block/absent").is_none());
        // Parent not found and no elements: the chain ends on the missing cube.
        let orphan = models.flatten("minecraft:block/orphan").expect("chain ended");
        assert_eq!(orphan.elements.len(), 1);
        assert_eq!(orphan.elements[0].to, [16.0; 3]);
        assert_eq!(orphan.resolve("particle").map(|t| t.value).as_deref(), Some(model::MISSING_TEXTURE));
        // Misspelled parent: the child keeps its geometry and its textures.
        let child = models.flatten("minecraft:block/child").expect("chain ended");
        assert_eq!(child.elements.len(), 1);
        assert_eq!(child.elements[0].to, [16.0, 8.0, 16.0]);
        assert_eq!(child.resolve("#all").map(|t| t.value).as_deref(), Some("minecraft:block/stone"));
        assert_eq!(child.resolve("particle").map(|t| t.value).as_deref(), Some(model::MISSING_TEXTURE));
        // Same rule for items.
        let item = models.flatten_item("minecraft:block/child").expect("item");
        assert!(matches!(&item.geometry, model::ItemGeometry::Elements(e) if e[0].to == [16.0, 8.0, 16.0]));
        assert!(models.flatten_item("minecraft:block/absent").is_none());
    }

    #[test]
    fn missing_cube_is_a_full_culled_block_with_missing_texture() {
        let mut textures = TextureTable { index: HashMap::new(), names: Vec::new() };
        textures.id("rustcraft:missing");
        let key = VariantKey { model: MISSING_MODEL, x: 0, y: 0, z: 0, uvlock: false };
        let q = bake_variant(&Model::missing(), &key, &mut textures, &HashMap::new(), &HashMap::new(), &mut Vec::new());
        assert_eq!(q.len(), 6);
        for (quad, d) in q.iter().zip(Dir::ALL) {
            assert_eq!((quad.texture, quad.cull, quad.dir, quad.layer, quad.ao, quad.tint), (0, Some(d), d, Layer::Solid, true, -1));
            assert!(quad.pos.iter().flatten().all(|&c| c == 0.0 || c == 1.0));
            let us: Vec<f32> = quad.uv.iter().map(|u| u[0]).collect();
            assert!(us.contains(&0.0) && us.contains(&1.0));
        }
        // The game's checkerboard: purple at the top right, black at the top left.
        let img = Image::missing(16);
        assert_eq!(&img.rgba[..4], &[0, 0, 0, 255]);
        assert_eq!(&img.rgba[15 * 4..16 * 4], &[248, 0, 248, 255]);
    }

    #[test]
    fn animation_clock_steps_and_fades() {
        let a = TextureAnimation { texture: 5, steps: vec![(5, 2), (9, 4)], interpolate: true };
        let mut c = AnimationClock::default();
        assert_eq!(c.frame(&a), (5, 9, 0));
        c.tick(&a);
        assert_eq!(c.frame(&a), (5, 9, 500));
        c.tick(&a);
        assert_eq!(c.frame(&a), (9, 5, 0));
        for _ in 0..3 {
            c.tick(&a);
        }
        assert_eq!(c.frame(&a), (9, 5, 750));
        c.tick(&a);
        assert_eq!(c.frame(&a), (5, 9, 0));
        let fixed = TextureAnimation { interpolate: false, ..a };
        assert_eq!(AnimationClock::default().frame(&fixed), (5, 5, 0));
    }
}
