//! Block models: JSON reading, inheritance through `parent`, resolution of the `#ref` textures.

use hashbrown::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::geom::{self, Dir, M3, V3};
use crate::source::{Assets, split_id};

#[derive(Debug, Clone)]
pub struct Face {
    pub uv: Option<[f32; 4]>,
    pub texture: String,
    pub cull: Option<Dir>,
    /// Quarter turns (0..4) applied to the vertex index to read the UVs.
    pub rotation: u8,
    pub tint: i32,
}

#[derive(Debug, Clone)]
pub struct ElementRotation {
    /// Origin as a fraction of a block.
    pub origin: V3,
    /// Rotation (and possible `rescale` scaling).
    pub matrix: M3,
}

#[derive(Debug, Clone)]
pub struct Element {
    pub from: V3,
    pub to: V3,
    pub rotation: Option<ElementRotation>,
    /// Shading direction forced on all faces (`shade_direction_override`), otherwise
    /// each face is shaded according to its own direction. The old boolean `shade` is no longer read
    /// by the game.
    pub shade_override: Option<Dir>,
    /// Emission of the element (`light_emission`, 0–15): light floor of its vertices.
    pub light_emission: u8,
    pub faces: [Option<Face>; 6],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureRef {
    pub value: String,
    pub force_translucent: bool,
}

#[derive(Debug, Default)]
struct RawModel {
    parent: Option<String>,
    textures: Vec<(String, TextureRef)>,
    elements: Option<Arc<Vec<Element>>>,
    ambient_occlusion: Option<bool>,
    /// `gui_light`: `Some(true)` for `front`, `Some(false)` for `side`.
    gui_front: Option<bool>,
    /// Display transformations per context (`DISPLAY_CONTEXTS`): rotation (degrees),
    /// translation (blocks, bounded), scale (bounded).
    display: [Option<Display>; DISPLAY_CONTEXTS.len()],
}

/// Display contexts of an item model, in the order of the `display` arrays.
pub const DISPLAY_CONTEXTS: [&str; 9] = [
    "gui",
    "ground",
    "thirdperson_righthand",
    "thirdperson_lefthand",
    "firstperson_righthand",
    "firstperson_lefthand",
    "head",
    "fixed",
    "on_shelf",
];

/// Display transformation of a context (`items.md` part 2 § 4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Display {
    pub rotation: V3,
    pub translation: V3,
    pub scale: V3,
}

/// Geometry of an item model.
#[derive(Debug, Clone)]
pub enum ItemGeometry {
    Elements(Arc<Vec<Element>>),
    /// `builtin/generated`: extrusion of the layers `layer0..4`.
    Generated,
}

/// Flattened item model.
#[derive(Debug, Clone)]
pub struct ItemModel {
    pub geometry: ItemGeometry,
    pub textures: HashMap<String, TextureRef>,
    /// Front GUI lighting (`front`); default `side`.
    pub gui_front: bool,
    pub display_gui: Option<Display>,
    /// Transformation of each context (`DISPLAY_CONTEXTS`), from the nearest ancestor that defines it.
    pub display: [Option<Display>; DISPLAY_CONTEXTS.len()],
}

impl ItemModel {
    pub fn resolve(&self, name: &str) -> Option<TextureRef> {
        Model { elements: Arc::new(Vec::new()), textures: self.textures.clone(), ambient_occlusion: true }.resolve(name)
    }
}

/// Flattened model: elements of the nearest ancestor that defines any, merged textures.
#[derive(Debug, Clone)]
pub struct Model {
    pub elements: Arc<Vec<Element>>,
    pub textures: HashMap<String, TextureRef>,
    /// Smooth lighting and ambient occlusion (`ambientocclusion` of the nearest ancestor that
    /// defines it, true by default).
    pub ambient_occlusion: bool,
}

/// Missing texture (purple and black checkerboard), outside any pack.
pub const MISSING_TEXTURE: &str = "rustcraft:missing";

/// Cube of the missing model: full block, six whole faces on `#missingno`, each culled
/// against a full neighbour on its side.
fn missing_element() -> Element {
    let face = |d: Dir| Face { uv: Some([0.0, 0.0, 16.0, 16.0]), texture: "#missingno".into(), cull: Some(d), rotation: 0, tint: -1 };
    Element {
        from: [0.0; 3],
        to: [16.0; 3],
        rotation: None,
        shade_override: None,
        light_emission: 0,
        faces: Dir::ALL.map(|d| Some(face(d))),
    }
}

/// Textures of the missing model: `missingno` (missing texture), `particle` → `#missingno`.
fn missing_textures() -> [(String, TextureRef); 2] {
    let r = |v: &str| TextureRef { value: v.into(), force_translucent: false };
    [("missingno".into(), r(MISSING_TEXTURE)), ("particle".into(), r("#missingno"))]
}

impl Model {
    /// The game's missing model (model or parent not found): full cube on the missing
    /// texture, smooth lighting.
    pub fn missing() -> Model {
        Model {
            elements: Arc::new(vec![missing_element()]),
            textures: missing_textures().into_iter().collect(),
            ambient_occlusion: true,
        }
    }

    /// Resolves `#name` step by step; `None` if the chain leads to no texture.
    pub fn resolve(&self, name: &str) -> Option<TextureRef> {
        let mut current = name.strip_prefix('#').unwrap_or(name).to_owned();
        for _ in 0..32 {
            let r = self.textures.get(&current)?;
            match r.value.strip_prefix('#') {
                Some(next) => current = next.to_owned(),
                None => return Some(r.clone()),
            }
        }
        None
    }
}

fn v3(v: &Value) -> Option<V3> {
    let a = v.as_array()?;
    Some([a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32, a.get(2)?.as_f64()? as f32])
}

fn parse_rotation(r: &Value) -> Option<ElementRotation> {
    let origin = v3(r.get("origin")?)?.map(|c| c / 16.0);
    let mut m = if let (Some(axis), Some(angle)) = (r.get("axis").and_then(Value::as_str), r.get("angle")) {
        let axis = match axis {
            "x" => geom::X,
            "y" => geom::Y,
            "z" => geom::Z,
            _ => return None,
        };
        let angle = angle.as_f64()? as f32;
        if angle == 0.0 { geom::IDENTITY } else { geom::rotation(axis, angle) }
    } else {
        let a = |k: &str| r.get(k).and_then(Value::as_f64).unwrap_or(0.0) as f32;
        // Euler: Z, then Y, then X (matrix Rz·Ry·Rx).
        geom::mul(
            &geom::rotation(geom::Z, a("z")),
            &geom::mul(&geom::rotation(geom::Y, a("y")), &geom::rotation(geom::X, a("x"))),
        )
    };
    if r.get("rescale").and_then(Value::as_bool).unwrap_or(false) && m != geom::IDENTITY {
        // Per-axis scale: 1 / largest component of the transformed unit axis, applied
        // before the rotation (M · S).
        let scale: Vec<f32> = [geom::X, geom::Y, geom::Z]
            .iter()
            .map(|&a| {
                let t = geom::mul_v(&m, a);
                1.0 / t.iter().fold(0.0f32, |acc, c| acc.max(c.abs()))
            })
            .collect();
        for row in &mut m {
            for (j, c) in row.iter_mut().enumerate() {
                *c *= scale[j];
            }
        }
    }
    Some(ElementRotation { origin, matrix: m })
}

fn parse_face(f: &Value) -> Option<Face> {
    let uv = f
        .get("uv")
        .and_then(Value::as_array)
        .and_then(|a| (a.len() == 4).then(|| [0, 1, 2, 3].map(|i| a[i].as_f64().unwrap_or(0.0) as f32)));
    let rotation = (f.get("rotation").and_then(Value::as_i64).unwrap_or(0).rem_euclid(360) / 90) as u8;
    Some(Face {
        uv,
        texture: f.get("texture")?.as_str()?.to_owned(),
        cull: f.get("cullface").and_then(Value::as_str).and_then(Dir::parse),
        rotation,
        tint: f.get("tintindex").and_then(Value::as_i64).unwrap_or(-1) as i32,
    })
}

fn parse_element(e: &Value) -> Option<Element> {
    let mut faces: [Option<Face>; 6] = Default::default();
    if let Some(obj) = e.get("faces").and_then(Value::as_object) {
        for (k, f) in obj {
            if let Some(d) = Dir::parse(k) {
                faces[d as usize] = parse_face(f);
            }
        }
    }
    Some(Element {
        from: v3(e.get("from")?)?,
        to: v3(e.get("to")?)?,
        rotation: e.get("rotation").and_then(parse_rotation),
        shade_override: e.get("shade_direction_override").and_then(Value::as_str).and_then(Dir::parse),
        light_emission: e.get("light_emission").and_then(Value::as_u64).unwrap_or(0).min(15) as u8,
        faces,
    })
}

fn parse_raw(json: &Value) -> RawModel {
    let mut m = RawModel {
        parent: json.get("parent").and_then(Value::as_str).map(str::to_owned),
        ambient_occlusion: json.get("ambientocclusion").and_then(Value::as_bool),
        gui_front: json.get("gui_light").and_then(Value::as_str).map(|l| l == "front"),
        display: {
            let mut d: [Option<Display>; DISPLAY_CONTEXTS.len()] = DISPLAY_CONTEXTS.map(|c| {
                json.pointer(&format!("/display/{c}")).map(|g| {
                    let v = |k: &str, d: V3| g.get(k).and_then(v3).unwrap_or(d);
                    Display {
                        rotation: v("rotation", [0.0; 3]),
                        translation: v("translation", [0.0; 3]).map(|c| (c / 16.0).clamp(-5.0, 5.0)),
                        scale: v("scale", [1.0; 3]).map(|c| c.clamp(-4.0, 4.0)),
                    }
                })
            });
            // Left hand absent: copy of the right hand (at load time of each file).
            if d[3].is_none() && json.pointer("/display").is_some() {
                d[3] = d[2];
            }
            if d[5].is_none() && json.pointer("/display").is_some() {
                d[5] = d[4];
            }
            d
        },
        ..Default::default()
    };
    if let Some(t) = json.get("textures").and_then(Value::as_object) {
        for (k, v) in t {
            let r = match v {
                Value::String(s) => TextureRef { value: s.clone(), force_translucent: false },
                Value::Object(o) => TextureRef {
                    value: o.get("sprite").and_then(Value::as_str).unwrap_or_default().to_owned(),
                    force_translucent: o.get("force_translucent").and_then(Value::as_bool).unwrap_or(false),
                },
                _ => continue,
            };
            m.textures.push((k.clone(), r));
        }
    }
    if let Some(els) = json.get("elements").and_then(Value::as_array) {
        m.elements = Some(Arc::new(els.iter().filter_map(parse_element).collect()));
    }
    m
}

/// Cache of the raw and flattened models, shared between threads.
pub struct Models<'a> {
    assets: &'a Assets,
    raw: Mutex<HashMap<String, Option<Arc<RawModel>>>>,
}

fn normalize(id: &str) -> String {
    let (ns, path) = split_id(id);
    format!("{ns}:{path}")
}

impl<'a> Models<'a> {
    pub fn new(assets: &'a Assets) -> Self {
        Models { assets, raw: Mutex::new(HashMap::new()) }
    }

    fn raw(&self, id: &str) -> Option<Arc<RawModel>> {
        let key = normalize(id);
        if let Some(r) = self.raw.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
            return r.clone();
        }
        let (ns, path) = split_id(&key);
        let parsed = self
            .assets
            .read(&format!("assets/{ns}/models/{path}.json"))
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .map(|j| Arc::new(parse_raw(&j)));
        self.raw.lock().unwrap_or_else(|p| p.into_inner()).insert(key, parsed.clone());
        parsed
    }

    /// Flattened model; `None` if not found (missing cube). Parent not found: like the
    /// game, the chain ends on the missing model (the child keeps its elements and its
    /// textures, the rest comes from the missing cube). Without elements in the whole chain (air, water,
    /// chest: particle only): empty geometry, nothing to draw.
    pub fn flatten(&self, id: &str) -> Option<Model> {
        let mut textures = HashMap::new();
        let mut elements = None;
        let mut ambient_occlusion = None;
        let mut current = Some(id.to_owned());
        let mut first = true;
        for _ in 0..64 {
            let Some(name) = current.take() else { break };
            let raw = match self.raw(&name) {
                Some(raw) => raw,
                None if first => return None,
                None => {
                    for (k, v) in missing_textures() {
                        textures.entry(k).or_insert(v);
                    }
                    elements = elements.or_else(|| Some(Arc::new(vec![missing_element()])));
                    break;
                }
            };
            first = false;
            for (k, v) in &raw.textures {
                textures.entry(k.clone()).or_insert_with(|| v.clone());
            }
            if elements.is_none() {
                elements = raw.elements.clone();
            }
            ambient_occlusion = ambient_occlusion.or(raw.ambient_occlusion);
            current = raw.parent.clone();
        }
        Some(Model { elements: elements.unwrap_or_default(), textures, ambient_occlusion: ambient_occlusion.unwrap_or(true) })
    }

    /// Flattened item model: geometry of the nearest (elements or `builtin/generated`),
    /// `gui_light` and `display.gui` of the nearest that defines them, merged textures.
    pub fn flatten_item(&self, id: &str) -> Option<ItemModel> {
        let mut textures = HashMap::new();
        let mut geometry = None;
        let mut gui_front = None;
        let mut display = [None; DISPLAY_CONTEXTS.len()];
        let mut current = Some(id.to_owned());
        let mut first = true;
        for _ in 0..64 {
            let Some(name) = current.take() else { break };
            if normalize(&name) == "minecraft:builtin/generated" {
                geometry.get_or_insert(ItemGeometry::Generated);
                gui_front = gui_front.or(Some(true));
                break;
            }
            let raw = match self.raw(&name) {
                Some(raw) => raw,
                // The requested model itself: not found (missing item).
                None if first => return None,
                // Parent not found: the chain ends on the missing model.
                None => {
                    for (k, v) in missing_textures() {
                        textures.entry(k).or_insert(v);
                    }
                    geometry.get_or_insert_with(|| ItemGeometry::Elements(Arc::new(vec![missing_element()])));
                    break;
                }
            };
            first = false;
            for (k, v) in &raw.textures {
                textures.entry(k.clone()).or_insert_with(|| v.clone());
            }
            if geometry.is_none() {
                geometry = raw.elements.clone().map(ItemGeometry::Elements);
            }
            gui_front = gui_front.or(raw.gui_front);
            for (d, r) in display.iter_mut().zip(raw.display) {
                *d = d.or(r);
            }
            current = raw.parent.clone();
        }
        Some(ItemModel {
            geometry: geometry.unwrap_or_else(|| ItemGeometry::Elements(Arc::new(Vec::new()))),
            textures,
            gui_front: gui_front.unwrap_or(false),
            display_gui: display[0],
            display,
        })
    }
}
