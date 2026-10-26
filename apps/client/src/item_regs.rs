//! Synchronized registries read by the item and armor renderer.
//!
//! - Armor trims: materials (`palette_id`) and patterns (`asset_id`, `decal`), in the server's
//!   network order; a registry received without data (known pack) takes those of the jar.
//!   Worn texture: `trims/entity/<layer type>/<pattern>` recolored by the material's palette,
//!   unless overridden by the equipment (`trim_overrides`: other texture, other palette, or
//!   no palette at all).
//! - Enchantments: `crossbow_charge_time` effect (crossbow charge duration).

use hashbrown::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, LazyLock, RwLock};

use rc_assets::palette::{self, Palette};
use rc_assets::source::Assets;
use rc_assets::texture::Image;
use rc_proto::Reader;
use rc_proto::nbt::Nbt;
use rc_proto::varint::read_varint;
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrimMaterial {
    /// Registry id (empty for a material given directly in the stack).
    pub id: String,
    pub palette: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrimPattern {
    pub id: String,
    pub asset: String,
    pub decal: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Trim {
    pub material: TrimMaterial,
    pub pattern: TrimPattern,
}

/// Trim override of a piece of equipment: condition (material, pattern), texture, palette.
#[derive(Clone, Debug, Default)]
struct Override {
    material: Option<String>,
    pattern: Option<String>,
    texture: Option<String>,
    palette: Option<String>,
}

#[derive(Default)]
struct Regs {
    assets: Option<Arc<Assets>>,
    def_materials: HashMap<String, TrimMaterial>,
    def_patterns: HashMap<String, TrimPattern>,
    /// `crossbow_charge_time` effect of the jar's enchantments.
    def_charge: HashMap<String, Value>,
    materials: Vec<TrimMaterial>,
    patterns: Vec<TrimPattern>,
    /// Enchantments in network order: possible charge effect.
    enchants: Vec<Option<Value>>,
    overrides: HashMap<String, Vec<Override>>,
    palettes: HashMap<String, Palette>,
    /// Base palette of the trim textures (`.png.mcmeta`), by texture path.
    bases: HashMap<String, String>,
    /// Worn textures already resolved: fingerprint → name in the atlas (`None`: not found).
    worn: HashMap<u64, Option<String>>,
}

static REGS: LazyLock<RwLock<Regs>> = LazyLock::new(|| RwLock::new(Regs::default()));

fn norm(id: &str) -> String {
    if id.contains(':') { id.to_owned() } else { format!("minecraft:{id}") }
}

fn split(id: &str) -> (&str, &str) {
    id.split_once(':').unwrap_or(("minecraft", id))
}

fn json(assets: &Assets, path: &str) -> Option<Value> {
    serde_json::from_slice(&assets.read(path)?).ok()
}

/// Data from the jar and the packs (at startup and on every resource reload).
pub fn load(assets: &Arc<Assets>) {
    let mut g = Regs { assets: Some(assets.clone()), ..Regs::default() };
    let name_of = |f: &str, dir: &str| {
        f.strip_prefix(dir).and_then(|n| n.strip_suffix(".json")).map(|n| format!("minecraft:{n}"))
    };
    let dir = "data/minecraft/trim_material/";
    for f in assets.list(dir, ".json") {
        let (Some(id), Some(v)) = (name_of(&f, dir), json(assets, &f)) else { continue };
        let palette = v["palette_id"].as_str().map(norm).unwrap_or_default();
        g.def_materials.insert(id.clone(), TrimMaterial { id, palette });
    }
    let dir = "data/minecraft/trim_pattern/";
    for f in assets.list(dir, ".json") {
        let (Some(id), Some(v)) = (name_of(&f, dir), json(assets, &f)) else { continue };
        let asset = v["asset_id"].as_str().map(norm).unwrap_or_default();
        let decal = v["decal"].as_bool().unwrap_or(false);
        g.def_patterns.insert(id.clone(), TrimPattern { id, asset, decal });
    }
    let dir = "data/minecraft/enchantment/";
    for f in assets.list(dir, ".json") {
        let (Some(id), Some(v)) = (name_of(&f, dir), json(assets, &f)) else { continue };
        if let Some(e) = v["effects"].get("minecraft:crossbow_charge_time") {
            g.def_charge.insert(id, e.clone());
        }
    }
    let dir = "assets/minecraft/equipment/";
    for f in assets.list(dir, ".json") {
        let (Some(id), Some(v)) = (name_of(&f, dir), json(assets, &f)) else { continue };
        let s = |o: &Value, k: &str| o[k].as_str().map(norm);
        let list: Vec<Override> = v["trim_overrides"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|o| Override {
                material: s(&o["when"], "material"),
                pattern: s(&o["when"], "pattern"),
                texture: s(o, "texture"),
                palette: s(o, "palette"),
            })
            .collect();
        if !list.is_empty() {
            g.overrides.insert(id, list);
        }
    }
    let dir = "assets/minecraft/textures/palettes/";
    for f in assets.list(dir, ".png") {
        let Some(path) = f.strip_prefix(dir).and_then(|n| n.strip_suffix(".png")) else { continue };
        let id = format!("minecraft:{path}");
        if let Some(p) = palette::load_palette(assets, &id) {
            g.palettes.insert(id, p);
        }
    }
    for f in assets.list("assets/minecraft/textures/trims/entity/", ".png") {
        if let Some(b) = palette::base_palette(assets, &f) {
            g.bases.insert(f, b);
        }
    }
    // Received registries stay (reload during play): data recomputed.
    let old = REGS.read().unwrap_or_else(|p| p.into_inner());
    g.materials = old.materials.clone();
    g.patterns = old.patterns.clone();
    g.enchants = old.enchants.clone();
    drop(old);
    *REGS.write().unwrap_or_else(|p| p.into_inner()) = g;
}

fn nbt_json(n: &Nbt) -> Value {
    match n {
        Nbt::Byte(v) => Value::from(*v),
        Nbt::Short(v) => Value::from(*v),
        Nbt::Int(v) => Value::from(*v),
        Nbt::Long(v) => Value::from(*v),
        Nbt::Float(v) => Value::from(*v),
        Nbt::Double(v) => Value::from(*v),
        Nbt::String(s) => Value::from(s.as_str()),
        Nbt::List(l) => Value::Array(l.iter().map(nbt_json).collect()),
        Nbt::Compound(c) => Value::Object(c.iter().map(|(k, v)| (k.clone(), nbt_json(v))).collect()),
        Nbt::ByteArray(a) => Value::Array(a.iter().map(|&b| Value::from(b as i8)).collect()),
        Nbt::IntArray(a) => Value::Array(a.iter().map(|&b| Value::from(b)).collect()),
        Nbt::LongArray(a) => Value::Array(a.iter().map(|&b| Value::from(b)).collect()),
    }
}

/// Synchronized registry received from the server.
pub fn set_registry(name: &str, entries: &rc_client::RegistryEntries) {
    let mut g = REGS.write().unwrap_or_else(|p| p.into_inner());
    match name {
        "minecraft:trim_material" => {
            g.materials = entries
                .iter()
                .map(|(id, n)| match n {
                    Some(n) => TrimMaterial {
                        id: id.clone(),
                        palette: n.get("palette_id").and_then(Nbt::as_str).map(norm).unwrap_or_default(),
                    },
                    None => g
                        .def_materials
                        .get(id)
                        .cloned()
                        .unwrap_or_else(|| TrimMaterial { id: id.clone(), ..Default::default() }),
                })
                .collect();
        }
        "minecraft:trim_pattern" => {
            g.patterns = entries
                .iter()
                .map(|(id, n)| match n {
                    Some(n) => TrimPattern {
                        id: id.clone(),
                        asset: n.get("asset_id").and_then(Nbt::as_str).map(norm).unwrap_or_default(),
                        decal: n.get("decal").and_then(Nbt::as_i64).is_some_and(|b| b != 0),
                    },
                    None => g
                        .def_patterns
                        .get(id)
                        .cloned()
                        .unwrap_or_else(|| TrimPattern { id: id.clone(), ..Default::default() }),
                })
                .collect();
        }
        "minecraft:enchantment" => {
            g.enchants = entries
                .iter()
                .map(|(id, n)| match n {
                    Some(n) => n.get("effects").and_then(|e| e.get("minecraft:crossbow_charge_time")).map(nbt_json),
                    None => g.def_charge.get(id).cloned(),
                })
                .collect();
        }
        _ => return,
    }
    g.worn.clear();
}

fn skip_component(r: &mut Reader<'_>) -> Option<()> {
    rc_proto::nbt::read(r).ok()?;
    Some(())
}

/// `trim` component (network bytes): material then pattern, each by reference to the registry
/// (index + 1) or given directly.
pub fn decode(b: &[u8]) -> Option<Trim> {
    let g = REGS.read().unwrap_or_else(|p| p.into_inner());
    let r = &mut Reader::new(b);
    let material = match read_varint(r).ok()? {
        0 => {
            let palette = norm(rc_proto::read_str(r, 32767).ok()?);
            skip_component(r)?;
            TrimMaterial { id: String::new(), palette }
        }
        v => g.materials.get(usize::try_from(v - 1).ok()?)?.clone(),
    };
    let pattern = match read_varint(r).ok()? {
        0 => {
            let asset = norm(rc_proto::read_str(r, 32767).ok()?);
            skip_component(r)?;
            let decal = r.u8().ok()? != 0;
            TrimPattern { id: String::new(), asset, decal }
        }
        v => g.patterns.get(usize::try_from(v - 1).ok()?)?.clone(),
    };
    Some(Trim { material, pattern })
}

/// Worn texture of a trim: path of the base texture and target palette (`None`:
/// base texture as is).
fn worn_source(g: &Regs, trim: &Trim, equip_asset: &str, layer_type: &str) -> (String, Option<String>) {
    let mut texture = trim.pattern.asset.clone();
    let mut pal = Some(trim.material.palette.clone());
    let key_is = |want: &Option<String>, have: &str| want.as_ref().is_none_or(|w| !have.is_empty() && w == have);
    for o in g.overrides.get(equip_asset).into_iter().flatten() {
        if key_is(&o.material, &trim.material.id) && key_is(&o.pattern, &trim.pattern.id) {
            texture = o.texture.clone().unwrap_or(texture);
            pal = o.palette.clone();
            break;
        }
    }
    let (ns, path) = split(&texture);
    (format!("assets/{ns}/textures/trims/entity/{layer_type}/{path}.png"), pal)
}

/// Atlas name of the worn texture of a trim, and the image to place there if `known` says
/// it is not there yet. `None`: no texture (base not found).
pub fn worn_texture(
    trim_bytes: &[u8],
    equip_asset: &str,
    layer_type: &str,
    known: impl Fn(&str) -> bool,
) -> Option<(String, Option<Image>)> {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (trim_bytes, equip_asset, layer_type).hash(&mut h);
    let key = h.finish();
    if let Some(found) = REGS.read().unwrap_or_else(|p| p.into_inner()).worn.get(&key) {
        let name = found.clone()?;
        if known(&name) {
            return Some((name, None));
        }
    }
    let trim = decode(trim_bytes);
    let mut g = REGS.write().unwrap_or_else(|p| p.into_inner());
    let Some(trim) = trim else {
        g.worn.insert(key, None);
        return None;
    };
    let (base, pal) = worn_source(&g, &trim, equip_asset, layer_type);
    let name = format!("#trim:{base}@{}", pal.as_deref().unwrap_or(""));
    if known(&name) {
        g.worn.insert(key, Some(name.clone()));
        return Some((name, None));
    }
    let img = g.assets.as_ref().and_then(|a| a.image(&base)).map(|i| i.first_frame());
    let Some(img) = img else {
        g.worn.insert(key, None);
        return None;
    };
    let mapped = match (&pal, g.bases.get(&base)) {
        (Some(p), Some(b)) => match (g.palettes.get(b), g.palettes.get(p)) {
            (Some(bp), Some(tp)) => palette::apply(&img, &palette::mapping(bp, tp)),
            _ => img,
        },
        _ => img,
    };
    g.worn.insert(key, Some(name.clone()));
    Some((name, Some(mapped)))
}

/// Id of the material of a `trim` component (model property `trim_material`).
pub fn material_id(trim_bytes: &[u8]) -> Option<String> {
    decode(trim_bytes).map(|t| t.material.id).filter(|id| !id.is_empty())
}

/// Value per enchantment level.
fn level_value(v: &Value, lvl: i32) -> f32 {
    if let Some(n) = v.as_f64() {
        return n as f32;
    }
    let f = |k: &str| v[k].as_f64().unwrap_or(0.0) as f32;
    match v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
        "linear" => f("base") + f("per_level_above_first") * (lvl - 1) as f32,
        "constant" => f("value"),
        "clamped" => level_value(&v["value"], lvl).clamp(f("min"), f("max")),
        "fraction" => {
            let d = level_value(&v["denominator"], lvl);
            if d == 0.0 { 0.0 } else { level_value(&v["numerator"], lvl) / d }
        }
        "levels_squared" => (lvl * lvl) as f32 + f("added"),
        "lookup" => {
            let vals = v["values"].as_array().map(Vec::as_slice).unwrap_or(&[]);
            match usize::try_from(lvl - 1).ok().and_then(|i| vals.get(i)) {
                Some(x) if lvl >= 1 => x.as_f64().unwrap_or(0.0) as f32,
                _ => level_value(&v["fallback"], lvl),
            }
        }
        _ => 0.0,
    }
}

/// Value effect applied to `v` (the binomial removal, random, has no effect here).
fn value_effect(e: &Value, lvl: i32, v: f32) -> f32 {
    match e["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
        "add" => v + level_value(&e["value"], lvl),
        "multiply" => v * level_value(&e["factor"], lvl),
        "set" => level_value(&e["value"], lvl),
        "all_of" => e["effects"].as_array().into_iter().flatten().fold(v, |acc, x| value_effect(x, lvl, acc)),
        _ => v,
    }
}

/// Charge duration of a crossbow carrying the `enchantments` component `ench` (network bytes):
/// ⌊max(0, 1.25 s modified) × 20⌋.
pub fn crossbow_charge(ench: Option<&[u8]>) -> i32 {
    let mut secs = 1.25f32;
    if let Some(b) = ench {
        let g = REGS.read().unwrap_or_else(|p| p.into_inner());
        let r = &mut Reader::new(b);
        let n = read_varint(r).unwrap_or(0).max(0);
        for _ in 0..n {
            let (Ok(id), Ok(lvl)) = (read_varint(r), read_varint(r)) else { break };
            if let Some(Some(e)) = usize::try_from(id).ok().and_then(|i| g.enchants.get(i)) {
                secs = value_effect(e, lvl, secs);
            }
        }
    }
    (secs.max(0.0) * 20.0).floor() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_charge_levels() {
        let qc = serde_json::json!({"type": "minecraft:add", "value": {"type": "minecraft:linear", "base": -0.25, "per_level_above_first": -0.25}});
        let secs = |lvl| (value_effect(&qc, lvl, 1.25).max(0.0) * 20.0).floor() as i32;
        assert_eq!([secs(1), secs(2), secs(3), secs(5), secs(6)], [20, 15, 10, 0, 0]);
    }

    #[test]
    fn override_replaces_palette_for_matching_material() {
        let mut g = Regs::default();
        g.overrides.insert(
            "minecraft:iron".into(),
            vec![Override {
                material: Some("minecraft:iron".into()),
                palette: Some("minecraft:trim/iron_darker".into()),
                ..Default::default()
            }],
        );
        let trim = |m: &str| Trim {
            material: TrimMaterial { id: m.into(), palette: format!("minecraft:trim/{}", &m[10..]) },
            pattern: TrimPattern { id: "minecraft:silence".into(), asset: "minecraft:silence".into(), decal: false },
        };
        let (base, pal) = worn_source(&g, &trim("minecraft:iron"), "minecraft:iron", "humanoid");
        assert_eq!(base, "assets/minecraft/textures/trims/entity/humanoid/silence.png");
        assert_eq!(pal.as_deref(), Some("minecraft:trim/iron_darker"));
        let (_, pal) = worn_source(&g, &trim("minecraft:gold"), "minecraft:iron", "humanoid_leggings");
        assert_eq!(pal.as_deref(), Some("minecraft:trim/gold"));
        // Material given directly (no id): never affected by an override.
        let mut direct = trim("minecraft:iron");
        direct.material.id.clear();
        assert_eq!(worn_source(&g, &direct, "minecraft:iron", "humanoid").1.as_deref(), Some("minecraft:trim/iron"));
    }
}
