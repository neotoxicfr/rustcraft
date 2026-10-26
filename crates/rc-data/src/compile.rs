//! Assembles `VersionData` from the files produced by the preparation: facts from
//! the extractor (`facts.json`) and reports of the official generator (`blocks.json`,
//! `packets.json`, `registries.json`). The two sources are cross-checked.

use hashbrown::HashMap;
use std::path::Path;

use serde_json::Value;

use crate::model::{Aabb, BlockInfo, Bound, PacketDef, Phase, Property, Registry, StateInfo, VersionData};
use crate::{Error, Result};

fn read_json(path: &Path) -> Result<Value> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    serde_json::from_str(&text).map_err(|e| Error::Format(format!("{} : {e}", path.display())))
}

fn bad(what: impl Into<String>) -> Error {
    Error::Format(what.into())
}

fn arr<'a>(v: &'a Value, key: &str) -> Result<&'a Vec<Value>> {
    v.get(key).and_then(Value::as_array).ok_or_else(|| bad(format!("array `{key}` missing")))
}

fn int(v: &Value) -> Result<i64> {
    v.as_i64().ok_or_else(|| bad(format!("integer expected: {v}")))
}

fn ints<T: TryFrom<i64>>(v: &Value, key: &str) -> Result<Vec<T>> {
    arr(v, key)?.iter().map(|x| T::try_from(int(x)?).map_err(|_| bad(format!("`{key}` out of bounds")))).collect()
}

/// Client-side facts (`client_facts.json`), one per state.
pub fn client_states(path: &Path, states: usize) -> Result<Vec<crate::ClientState>> {
    let v = read_json(path)?;
    let skip: Vec<u8> = ints(&v, "skip_self")?;
    let kinds: Vec<Vec<u8>> = (0..4).map(|l| ints(&v, &format!("tint_kind_{l}"))).collect::<Result<_>>()?;
    let values: Vec<Vec<u32>> = (0..4)
        .map(|l| arr(&v, &format!("tint_value_{l}"))?.iter().map(|x| Ok(int(x)? as u32)).collect::<Result<Vec<u32>>>())
        .collect::<Result<_>>()?;
    let u32s = |key: &str| arr(&v, key)?.iter().map(|x| Ok(int(x)? as u32)).collect::<Result<Vec<u32>>>();
    let (particle_kind, particle_value): (Vec<u8>, _) = (ints(&v, "particle_tint_kind")?, u32s("particle_tint_value")?);
    let (dust_kind, dust_value): (Vec<u8>, _) = (ints(&v, "dust_kind")?, u32s("dust_value")?);
    let entity: Vec<Vec<u32>> = (0..4).map(|l| u32s(&format!("entity_tint_{l}"))).collect::<Result<_>>()?;
    if skip.len() != states
        || kinds.iter().chain([&particle_kind, &dust_kind]).any(|k| k.len() != states)
        || values.iter().chain([&particle_value, &dust_value]).chain(&entity).any(|k| k.len() != states)
    {
        return Err(bad(format!("{}: number of states differs from {states}", path.display())));
    }
    Ok((0..states)
        .map(|i| crate::ClientState {
            skip_self: skip[i],
            tint_kind: [kinds[0][i], kinds[1][i], kinds[2][i], kinds[3][i]],
            tint_value: [values[0][i], values[1][i], values[2][i], values[3][i]],
            particle_tint: (particle_kind[i], particle_value[i]),
            dust: (dust_kind[i], dust_value[i]),
            entity_tint: [entity[0][i], entity[1][i], entity[2][i], entity[3][i]],
        })
        .collect())
}

/// `reports`: folder containing `blocks.json`, `packets.json`, `registries.json`.
pub fn compile(facts_path: &Path, reports: &Path, protocol: i32) -> Result<VersionData> {
    let facts = read_json(facts_path)?;
    let blocks_report = read_json(&reports.join("blocks.json"))?;
    let packets_report = read_json(&reports.join("packets.json"))?;
    let registries_report = read_json(&reports.join("registries.json"))?;

    let mut d = VersionData {
        version: facts.get("version").and_then(Value::as_str).ok_or_else(|| bad("version missing"))?.to_owned(),
        protocol,
        ..Default::default()
    };

    // Shapes.
    for shape in arr(&facts, "shapes")? {
        let start = d.aabbs.len() as u32;
        for b in shape.as_array().ok_or_else(|| bad("shape: array expected"))? {
            let c: Vec<f64> = b
                .as_array()
                .filter(|c| c.len() == 6)
                .ok_or_else(|| bad("box: 6 numbers expected"))?
                .iter()
                .map(|x| x.as_f64().ok_or_else(|| bad("box: number expected")))
                .collect::<Result<_>>()?;
            d.aabbs.push(Aabb { min: [c[0], c[1], c[2]], max: [c[3], c[4], c[5]] });
        }
        d.shapes.push((start, d.aabbs.len() as u32));
    }
    if d.shapes.first().is_none_or(|&(a, b)| a != b) {
        return Err(bad("shape 0 must be empty"));
    }

    d.fluids = arr(&facts, "fluids")?.iter().map(|f| f.as_str().unwrap_or_default().to_owned()).collect();

    // States.
    let n = facts.get("state_count").map(int).transpose()?.ok_or_else(|| bad("state_count missing"))? as usize;
    let collision: Vec<u16> = ints(&facts, "collision")?;
    let outline: Vec<u16> = ints(&facts, "outline")?;
    let occlusion: Vec<u16> = ints(&facts, "occlusion")?;
    let support: Vec<u16> = ints(&facts, "support")?;
    let emission: Vec<u8> = ints(&facts, "light_emission")?;
    let dampening: Vec<u8> = ints(&facts, "light_dampening")?;
    let flag_bits: Vec<u16> = ints(&facts, "flags")?;
    let fluid: Vec<u8> = ints(&facts, "fluid")?;
    let fluid_amount: Vec<u8> = ints(&facts, "fluid_amount")?;
    let seed_offset: Vec<u8> = ints(&facts, "seed_offset")?;
    let sturdy: Vec<u8> = ints(&facts, "sturdy")?;
    let light: Vec<u8> = ints(&facts, "lightx")?;
    let sound: Vec<u8> = ints(&facts, "sound_type")?;
    let map_color: Vec<u8> = ints(&facts, "map_color")?;
    for t in arr(&facts, "sound_types")? {
        let a = t.as_array().filter(|a| a.len() == 7).ok_or_else(|| bad("sound set: 7 fields expected"))?;
        let s = |i: usize| a[i].as_str().unwrap_or_default().to_owned();
        d.sound_types.push(crate::SoundType {
            volume: a[0].as_f64().unwrap_or(1.0) as f32,
            pitch: a[1].as_f64().unwrap_or(1.0) as f32,
            break_sound: s(2),
            step: s(3),
            place: s(4),
            hit: s(5),
            fall: s(6),
        });
    }
    d.hardness = arr(&facts, "hardness")?
        .iter()
        .map(|x| x.as_f64().map(|v| v as f32).ok_or_else(|| bad(format!("hardness: number expected: {x}"))))
        .collect::<Result<_>>()?;
    let needs_tool: Vec<u8> = ints(&facts, "needs_tool")?;
    d.needs_tool = needs_tool.iter().map(|&v| v != 0).collect();
    d.sim = ints(&facts, "sim")?;
    d.instrument = ints(&facts, "instrument")?;
    d.blast = arr(&facts, "blast")?
        .iter()
        .map(|x| x.as_f64().map(|v| v as f32).ok_or_else(|| bad(format!("resistance: number expected: {x}"))))
        .collect::<Result<_>>()?;
    if sound.iter().any(|&s| usize::from(s) >= d.sound_types.len()) {
        return Err(bad("sound set index outside the table"));
    }
    for (name, len) in [
        ("sound_type", sound.len()),
        ("map_color", map_color.len()),
        ("hardness", d.hardness.len()),
        ("needs_tool", d.needs_tool.len()),
        ("sim", d.sim.len()),
        ("blast", d.blast.len()),
        ("instrument", d.instrument.len()),
        ("collision", collision.len()),
        ("outline", outline.len()),
        ("occlusion", occlusion.len()),
        ("support", support.len()),
        ("light_emission", emission.len()),
        ("light_dampening", dampening.len()),
        ("flags", flag_bits.len()),
        ("fluid", fluid.len()),
        ("fluid_amount", fluid_amount.len()),
        ("seed_offset", seed_offset.len()),
        ("sturdy", sturdy.len()),
        ("lightx", light.len()),
    ] {
        if len != n {
            return Err(bad(format!("`{name}`: {len} entries for {n} states")));
        }
    }
    let shape_count = d.shapes.len();
    if collision.iter().chain(&outline).chain(&occlusion).chain(&support).any(|&s| s as usize >= shape_count) {
        return Err(bad("shape index outside the table"));
    }
    d.states = (0..n)
        .map(|i| StateInfo {
            block: u16::MAX,
            collision: collision[i],
            outline: outline[i],
            occlusion: occlusion[i],
            support: support[i],
            flags: flag_bits[i],
            light_emission: emission[i],
            light_dampening: dampening[i],
            fluid: fluid[i],
            fluid_amount: fluid_amount[i],
            values: 0,
            seed_offset: seed_offset[i],
            sturdy: sturdy[i],
            light: light[i],
            sound: sound[i],
            map_color: map_color[i],
        })
        .collect();

    // Blocks, cross-checked with the official report; properties interned.
    let report_blocks = blocks_report.as_object().ok_or_else(|| bad("blocks.json: object expected"))?;
    let mut prop_index: HashMap<(String, Vec<String>), u16> = HashMap::new();
    let mut values_at: Vec<Option<Vec<u8>>> = vec![None; n];
    for (bi, b) in arr(&facts, "blocks")?.iter().enumerate() {
        let name = b.get("name").and_then(Value::as_str).ok_or_else(|| bad("block without a name"))?.to_owned();
        let first = int(&b["first"])? as u32;
        let count = int(&b["count"])? as u32;
        let default = int(&b["default"])? as u32;
        let r = report_blocks.get(&name).ok_or_else(|| bad(format!("{name} absent from the report")))?;
        let r_states = arr(r, "states")?;
        if r_states.len() != count as usize {
            return Err(bad(format!("{name}: {count} states extracted, {} in the report", r_states.len())));
        }

        let mut props = Vec::new();
        let mut prop_values: Vec<(String, Vec<String>)> = Vec::new();
        if let Some(obj) = r.get("properties").and_then(Value::as_object) {
            for (pname, vals) in obj {
                let vals: Vec<String> = vals
                    .as_array()
                    .ok_or_else(|| bad("property values: array expected"))?
                    .iter()
                    .map(|v| v.as_str().unwrap_or_default().to_owned())
                    .collect();
                let key = (pname.clone(), vals.clone());
                let next = prop_index.len() as u16;
                let idx = *prop_index.entry(key).or_insert_with(|| {
                    d.properties.push(Property { name: pname.clone(), values: vals.clone() });
                    next
                });
                props.push(idx);
                prop_values.push((pname.clone(), vals));
            }
        }

        let mut saw_default = false;
        for s in r_states {
            let id = int(&s["id"])? as u32;
            if id < first || id >= first + count {
                return Err(bad(format!("{name}: state {id} outside the extracted range")));
            }
            if s.get("default").and_then(Value::as_bool) == Some(true) {
                if id != default {
                    return Err(bad(format!("{name}: default state {id} ≠ {default}")));
                }
                saw_default = true;
            }
            let mut vals = Vec::with_capacity(prop_values.len());
            for (pname, allowed) in &prop_values {
                let v = s.get("properties").and_then(|p| p.get(pname)).and_then(Value::as_str);
                let i = v
                    .and_then(|v| allowed.iter().position(|a| a == v))
                    .ok_or_else(|| bad(format!("{name}: missing value for {pname}")))?;
                vals.push(i as u8);
            }
            let st = &mut d.states[id as usize];
            st.block = bi as u16;
            values_at[id as usize] = Some(vals);
        }
        if !saw_default {
            return Err(bad(format!("{name}: no default state in the report")));
        }
        d.blocks.push(BlockInfo {
            name,
            first_state: first,
            state_count: count,
            default_state: default,
            friction: b["friction"].as_f64().unwrap_or(0.6) as f32,
            speed_factor: b["speed"].as_f64().unwrap_or(1.0) as f32,
            jump_factor: b["jump"].as_f64().unwrap_or(1.0) as f32,
            bounce: b["bounce"].as_f64().unwrap_or(0.0) as f32,
            inside: b["inside"].as_u64().unwrap_or(0) as u8,
            offset_kind: b["offset_kind"].as_u64().unwrap_or(0) as u8,
            offset_h: b["offset_h"].as_f64().unwrap_or(0.0) as f32,
            offset_v: b["offset_v"].as_f64().unwrap_or(0.0) as f32,
            dynamic_shape: b["dynamic_shape"].as_bool().unwrap_or(false),
            properties: props,
        });
    }
    for (id, vals) in values_at.into_iter().enumerate() {
        let vals = vals.ok_or_else(|| bad(format!("state {id} without a block")))?;
        d.states[id].values = d.state_values.len() as u32;
        d.state_values.extend(vals);
    }

    // Packets.
    for phase in Phase::ALL {
        for bound in Bound::ALL {
            let Some(m) = packets_report.get(phase.report_key()).and_then(|p| p.get(bound.report_key())) else {
                continue;
            };
            for (name, e) in m.as_object().ok_or_else(|| bad("packets.json: object expected"))? {
                let id = int(&e["protocol_id"])? as i32;
                d.packets.push(PacketDef { phase, bound, id, name: name.clone() });
            }
        }
    }
    d.packets.sort_by_key(|p| (p.phase, p.bound, p.id));

    // Built-in registries.
    for (rname, r) in registries_report.as_object().ok_or_else(|| bad("registries.json: object expected"))? {
        let entries = r.get("entries").and_then(Value::as_object).ok_or_else(|| bad("registry without entries"))?;
        let mut v: Vec<Option<String>> = vec![None; entries.len()];
        for (ename, e) in entries {
            let id = int(&e["protocol_id"])? as usize;
            let slot = v.get_mut(id).ok_or_else(|| bad(format!("{rname}: identifier {id} out of bounds")))?;
            *slot = Some(ename.clone());
        }
        let entries = v.into_iter().collect::<Option<Vec<_>>>().ok_or_else(|| bad(format!("{rname}: holes")))?;
        d.registries.push(Registry::new(rname.clone(), entries));
    }
    d.registries.sort_by(|a, b| a.name.cmp(&b.name));
    d.items = read_items(reports, &d)?;
    read_potions(&facts, &mut d);
    Ok(d)
}

/// Entity models of the extractor (`ModelExtractor.java`).
pub fn entity_models(path: &Path) -> Result<Vec<crate::EntityLayer>> {
    let v = read_json(path)?;
    let f = |x: &Value| x.as_f64().unwrap_or(0.0) as f32;
    let arr3 = |x: &Value| [f(&x[0]), f(&x[1]), f(&x[2])];
    fn walk(p: &Value, parent: u32, out: &mut Vec<crate::EntityPart>, f: &dyn Fn(&Value) -> f32) {
        let me = out.len() as u32;
        let pose: Vec<f32> = p["pose"].as_array().map(|a| a.iter().map(f).collect()).unwrap_or_default();
        let a3 = |x: &Value| [f(&x[0]), f(&x[1]), f(&x[2])];
        out.push(crate::EntityPart {
            name: p["name"].as_str().unwrap_or_default().to_owned(),
            parent,
            pose: std::array::from_fn(|i| pose.get(i).copied().unwrap_or(if i >= 6 { 1.0 } else { 0.0 })),
            cubes: p["cubes"]
                .as_array()
                .map(|cs| {
                    cs.iter()
                        .map(|c| crate::EntityCube {
                            uv: [f(&c["uv"][0]), f(&c["uv"][1])],
                            from: a3(&c["from"]),
                            size: a3(&c["size"]),
                            grow: a3(&c["grow"]),
                            mirror: c["mirror"].as_bool().unwrap_or(false),
                            tex_scale: [f(&c["tex_scale"][0]), f(&c["tex_scale"][1])],
                            faces: c["faces"].as_u64().unwrap_or(63) as u8,
                        })
                        .collect()
                })
                .unwrap_or_default(),
        });
        for c in p["children"].as_array().into_iter().flatten() {
            walk(c, me, out, f);
        }
    }
    let _ = arr3;
    let mut layers: Vec<crate::EntityLayer> = arr(&v, "layers")?
        .iter()
        .map(|l| {
            let mut parts = Vec::new();
            walk(&l["root"], u32::MAX, &mut parts, &f);
            crate::EntityLayer {
                name: l["name"].as_str().unwrap_or_default().to_owned(),
                texture: [l["texture"][0].as_u64().unwrap_or(64) as u16, l["texture"][1].as_u64().unwrap_or(64) as u16],
                parts,
            }
        })
        .collect();
    layers.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(layers)
}

/// Keyframe animations of the extractor (`ModelExtractor.java`), sorted by name.
pub fn entity_animations(path: &Path) -> Result<Vec<crate::KeyframeAnimation>> {
    let v = read_json(path)?;
    let f = |x: &Value| x.as_f64().unwrap_or(0.0) as f32;
    let a3 = |x: &Value| [f(&x[0]), f(&x[1]), f(&x[2])];
    let mut out = arr(&v, "animations")?
        .iter()
        .map(|a| {
            let channels = arr(a, "channels")?
                .iter()
                .map(|c| {
                    let target = match c["target"].as_u64() {
                        Some(0) => crate::AnimTarget::Position,
                        Some(1) => crate::AnimTarget::Rotation,
                        Some(2) => crate::AnimTarget::Scale,
                        _ => return Err(bad("animation: unknown target")),
                    };
                    let keyframes = arr(c, "keyframes")?
                        .iter()
                        .map(|k| crate::Keyframe {
                            time: f(&k["t"]),
                            pre: a3(&k["pre"]),
                            post: a3(&k["post"]),
                            catmull: k["catmull"].as_bool().unwrap_or(false),
                        })
                        .collect::<Vec<_>>();
                    if keyframes.is_empty() {
                        return Err(bad("animation: channel without a keyframe"));
                    }
                    Ok(crate::AnimChannel {
                        part: c["part"].as_str().unwrap_or_default().to_owned(),
                        target,
                        keyframes,
                    })
                })
                .collect::<Result<_>>()?;
            Ok(crate::KeyframeAnimation {
                name: a["name"].as_str().unwrap_or_default().to_owned(),
                length: f(&a["length"]),
                looping: a["looping"].as_bool().unwrap_or(false),
                channels,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Potions and effect colours of the extractor, stored by registry identifier.
fn read_potions(facts: &Value, d: &mut VersionData) {
    let names = |r: &str| d.registry(r).map(|r| r.entries.clone()).unwrap_or_default();
    let effects = names("minecraft:mob_effect");
    let potion_names = names("minecraft:potion");
    let types = names("minecraft:entity_type");
    let colors = &facts["effect_colors"];
    // 24-bit colour; the high byte carries the category (0 beneficial, 1 harmful, 2 neutral).
    let cats = &facts["effect_categories"];
    d.effect_colors = effects
        .iter()
        .map(|e| {
            (colors[e].as_i64().unwrap_or(0) as u32 & 0xff_ffff) | (cats[e].as_u64().unwrap_or(0) as u32 & 3) << 24
        })
        .collect();
    let dims = &facts["entity_dims"];
    d.entity_dims = types
        .iter()
        .map(|t| {
            let v = |i: usize| dims[t][i].as_f64().unwrap_or(0.0) as f32;
            [v(0), v(1), v(2), v(3)]
        })
        .collect();
    let potions = &facts["potions"];
    d.potions = potion_names
        .iter()
        .map(|p| {
            potions[p]
                .as_array()
                .map(|list| {
                    list.iter()
                        .filter_map(|e| {
                            let name = e[0].as_str()?;
                            Some(crate::PotionEffect {
                                effect: effects.iter().position(|x| x == name)? as u16,
                                amplifier: e[1].as_u64()?.min(255) as u8,
                                visible: e[2].as_bool()?,
                                duration: e[3].as_i64().unwrap_or(0) as i32,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect();
}

/// Default components of each item (`reports/minecraft/components/item/<id>.json`), in
/// network identifier order; report absent (old generator): empty table.
fn read_items(reports: &Path, d: &VersionData) -> Result<Vec<crate::ItemInfo>> {
    let Some(reg) = d.registry("minecraft:item") else { return Ok(Vec::new()) };
    let dir = reports.join("minecraft").join("components").join("item");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let block_ids: HashMap<&str, u32> = d.blocks.iter().enumerate().map(|(i, b)| (b.name.as_str(), i as u32)).collect();
    let empty = Registry::new("", Vec::new());
    let component_types = d.data_component_types().unwrap_or(&empty);
    let block_set = |v: &Value| -> crate::BlockSet {
        let name = |s: &str| if s.contains(':') { s.to_owned() } else { format!("minecraft:{s}") };
        match v {
            Value::String(s) if s.starts_with('#') => crate::BlockSet::Tag(name(&s[1..])),
            Value::String(s) => crate::BlockSet::Blocks(block_ids.get(name(s).as_str()).copied().into_iter().collect()),
            Value::Array(a) => crate::BlockSet::Blocks(
                a.iter().filter_map(|x| x.as_str()).filter_map(|s| block_ids.get(name(s).as_str()).copied()).collect(),
            ),
            _ => crate::BlockSet::Blocks(Vec::new()),
        }
    };
    let anim = |v: &Value| -> [u8; 2] {
        let kind = match v["type"].as_str() {
            Some("none") => 0,
            Some("stab") => 2,
            _ => 1,
        };
        [kind, v["duration"].as_u64().unwrap_or(6).min(255) as u8]
    };
    let use_anim = |s: Option<&str>| -> u8 {
        let names = [
            "none",
            "eat",
            "drink",
            "block",
            "bow",
            "trident",
            "crossbow",
            "spyglass",
            "toot_horn",
            "brush",
            "bundle",
            "spear",
        ];
        s.and_then(|s| names.iter().position(|n| *n == s)).map_or(1, |i| i as u8)
    };
    let sound = |v: &Value, default: &str| -> String {
        v.as_str().or_else(|| v["sound_id"].as_str()).unwrap_or(default).to_owned()
    };
    let one = |name: &String| {
        let path = dir.join(format!("{}.json", name.strip_prefix("minecraft:").unwrap_or(name)));
        let v = read_json(&path)?;
        let c = &v["components"];
        let get = |k: &str| &c[format!("minecraft:{k}")];
        let tool = get("tool").as_object().map(|t| crate::Tool {
            rules: t
                .get("rules")
                .and_then(Value::as_array)
                .map(|rs| {
                    rs.iter()
                        .map(|r| crate::ToolRule {
                            blocks: block_set(&r["blocks"]),
                            speed: r["speed"].as_f64().map(|s| s as f32),
                            correct: r["correct_for_drops"].as_bool(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            default_speed: t.get("default_mining_speed").and_then(Value::as_f64).unwrap_or(1.0) as f32,
            damage_per_block: t.get("damage_per_block").and_then(Value::as_u64).unwrap_or(1) as u32,
            creative_destroy: t.get("can_destroy_blocks_in_creative").and_then(Value::as_bool).unwrap_or(true),
        });
        let consumable = get("consumable").as_object().map(|_| {
            let k = get("consumable");
            crate::Consumable {
                seconds: k["consume_seconds"].as_f64().unwrap_or(1.6) as f32,
                animation: use_anim(k["animation"].as_str()),
                sound: sound(&k["sound"], "minecraft:entity.generic.eat"),
                particles: k["has_consume_particles"].as_bool().unwrap_or(true),
            }
        });
        // Default attribute modifiers: direct list, or object with a `modifiers` key.
        let slot_of = |s: &str| {
            ["any", "mainhand", "offhand", "hand", "feet", "legs", "chest", "head", "armor", "body", "saddle"]
                .iter()
                .position(|n| *n == s)
                .unwrap_or(0) as u8
        };
        let mods = get("attribute_modifiers");
        let mods = mods.get("modifiers").unwrap_or(mods);
        let attributes = mods
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| {
                Some(crate::AttributeMod {
                    attribute: m["type"].as_str()?.trim_start_matches("minecraft:").to_owned(),
                    id: m["id"].as_str()?.to_owned(),
                    amount: m["amount"].as_f64()?,
                    operation: match m["operation"].as_str() {
                        Some("add_multiplied_base") => 1,
                        Some("add_multiplied_total") => 2,
                        _ => 0,
                    },
                    slot: slot_of(m["slot"].as_str().unwrap_or("any")),
                })
            })
            .collect();
        let range = get("attack_range");
        let rf = |k: &str, def: f64| range[k].as_f64().unwrap_or(def) as f32;
        let effects = get("use_effects");
        Ok(crate::ItemInfo {
            tool,
            consumable,
            food: get("food").as_object().map(|f| f.get("can_always_eat").and_then(Value::as_bool).unwrap_or(false)),
            attack_range: range.as_object().map(|_| {
                [
                    rf("min_reach", 0.0),
                    rf("max_reach", 3.0),
                    rf("min_creative_reach", 0.0),
                    rf("max_creative_reach", 5.0),
                    rf("hitbox_margin", 0.3),
                    rf("mob_factor", 1.0),
                ]
            }),
            min_attack_charge: get("minimum_attack_charge").as_f64().unwrap_or(0.0) as f32,
            piercing: !get("piercing_weapon").is_null(),
            kinetic: !get("kinetic_weapon").is_null(),
            kinetic_params: get("kinetic_weapon").as_object().map(|k| {
                let int = |v: Option<&Value>, d: i64| v.and_then(Value::as_i64).unwrap_or(d) as i32;
                let flt = |v: Option<&Value>, d: f64| v.and_then(Value::as_f64).unwrap_or(d) as f32;
                let cond = |name: &str| {
                    let c = k.get(name)?.as_object()?;
                    Some((
                        int(c.get("max_duration_ticks"), 0),
                        flt(c.get("min_speed"), 0.0),
                        flt(c.get("min_relative_speed"), 0.0),
                    ))
                };
                crate::KineticParams {
                    contact_cooldown: int(k.get("contact_cooldown_ticks"), 10),
                    delay: int(k.get("delay_ticks"), 0),
                    conditions: [cond("dismount_conditions"), cond("knockback_conditions"), cond("damage_conditions")],
                    forward_movement: flt(k.get("forward_movement"), 0.0),
                    damage_multiplier: flt(k.get("damage_multiplier"), 1.0),
                }
            }),
            blocks_attacks: !get("blocks_attacks").is_null(),
            attack_anim: anim(get("attack_animation")),
            interact_anim: anim(get("interact_animation")),
            use_sprint: effects["can_sprint"].as_bool().unwrap_or(false),
            use_speed: effects["speed_multiplier"].as_f64().unwrap_or(0.2) as f32,
            max_stack_size: get("max_stack_size").as_u64().unwrap_or(64).min(99) as u8,
            max_damage: get("max_damage").as_u64().unwrap_or(0) as u32,
            rarity: match get("rarity").as_str() {
                Some("uncommon") => 1,
                Some("rare") => 2,
                Some("epic") => 3,
                _ => 0,
            },
            name_key: get("item_name")["translate"].as_str().unwrap_or_default().to_owned(),
            model: get("item_model").as_str().unwrap_or_default().to_owned(),
            glint: match get("enchantment_glint_override").as_bool() {
                None => 0,
                Some(false) => 1,
                Some(true) => 2,
            },
            equip_slot: match get("equippable")["slot"].as_str() {
                Some("mainhand") => 1,
                Some("feet") => 2,
                Some("legs") => 3,
                Some("chest") => 4,
                Some("head") => 5,
                Some("offhand") => 6,
                Some("body") => 7,
                Some("saddle") => 8,
                _ => 0,
            },
            equip_asset: get("equippable")["asset_id"].as_str().unwrap_or_default().to_owned(),
            equip_overlay: get("equippable")["camera_overlay"].as_str().unwrap_or_default().to_owned(),
            attributes,
            components: c
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(k, _)| component_types.id(k).map(|i| i as u16))
                .collect(),
        })
    };
    // ≈ 1,500 reports read and parsed: in parallel, stored in identifier order
    // (`RUSTCRAFT_OPT_PREPPAR=0`: in series, old path; only the returned error may differ
    // if several reports are unreadable).
    if prep_par() {
        use rayon::prelude::*;
        reg.entries.par_iter().map(one).collect()
    } else {
        reg.entries.iter().map(one).collect()
    }
}

/// Preparation reads in parallel (`RUSTCRAFT_OPT_PREPPAR=0`: in series).
pub(crate) fn prep_par() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_str!("RUSTCRAFT_OPT_PREPPAR") != Some("0"))
}

/// Creative tabs of the extractor (`CreativeExtractor.java`).
pub fn creative(path: &Path) -> Result<crate::CreativeData> {
    use crate::{CreativeEntry, CreativeKind, CreativeStack, CreativeTab};
    let v = read_json(path)?;
    let s = |x: &Value| x.as_str().unwrap_or_default().to_owned();
    let hex = |h: &str| -> Result<Vec<u8>> {
        (0..h.len() / 2)
            .map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).map_err(|_| bad("creative: invalid hexadecimal")))
            .collect()
    };
    let stack = |x: &Value| -> Result<CreativeStack> {
        Ok(CreativeStack {
            item: s(&x["item"]),
            visibility: x["v"].as_u64().unwrap_or(0) as u8,
            features: x["f"].as_array().into_iter().flatten().map(s).collect(),
            added: x["c"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| Ok((s(&c[0]), hex(c[1].as_str().unwrap_or_default())?)))
                .collect::<Result<_>>()?,
            removed: x["r"].as_array().into_iter().flatten().map(s).collect(),
            banner: x["banner"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|l| (s(&l[0]), l[1].as_u64().unwrap_or(0) as u8))
                .collect(),
        })
    };
    let tabs = arr(&v, "tabs")?
        .iter()
        .map(|t| {
            let kind = match t["type"].as_str() {
                Some("category") => CreativeKind::Category,
                Some("hotbar") => CreativeKind::Hotbar,
                Some("search") => CreativeKind::Search,
                Some("inventory") => CreativeKind::Inventory,
                _ => return Err(bad("creative: unknown tab type")),
            };
            let entries = arr(t, "entries")?
                .iter()
                .map(|e| match e.get("dyn") {
                    Some(d) => Ok(CreativeEntry::Dynamic(s(d))),
                    None => stack(e).map(CreativeEntry::Stack),
                })
                .collect::<Result<_>>()?;
            Ok(CreativeTab {
                name: s(&t["name"]),
                title: s(&t["title"]),
                row: t["row"].as_u64().unwrap_or(0) as u8,
                column: t["column"].as_u64().unwrap_or(0) as u8,
                kind,
                right: t["right"].as_bool().unwrap_or(false),
                scroll: t["scroll"].as_bool().unwrap_or(true),
                show_title: t["show_title"].as_bool().unwrap_or(true),
                background: s(&t["background"]),
                icon: stack(&t["icon"])?,
                entries,
            })
        })
        .collect::<Result<_>>()?;
    let triple = |key: &str| -> Result<Vec<(String, u8, u8)>> {
        Ok(arr(&v, key)?
            .iter()
            .map(|p| (s(&p[0]), p[1].as_u64().unwrap_or(0) as u8, p[2].as_u64().unwrap_or(0) as u8))
            .collect())
    };
    Ok(crate::CreativeData { tabs, paintings: triple("paintings")?, enchantments: triple("enchantments")? })
}
