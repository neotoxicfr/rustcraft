//! Items: definitions `assets/<ns>/items/*.json`, evaluation in GUI context, baking of the
//! models (extrusion of the `builtin/generated` layers, elements) — `docs/specs/items.md`.

use hashbrown::HashMap;
use std::borrow::Cow;
use std::cell::Cell;
use std::sync::OnceLock;

use serde_json::Value;

use crate::geom::{Dir, V3};
use crate::model::{ItemGeometry, ItemModel, Models};
use crate::source::{Assets, split_id};
use crate::texture::{self, Image};

/// Optimisation switches of the items (`RUSTCRAFT_OPT_<NAME>=0`: old path), read once.
#[derive(Clone, Copy, Debug)]
pub struct Opt {
    /// Evaluation without allocation: `select` values borrowed, identifiers compared
    /// without a normalised string, tints inline, identifiers without namespace on the stack.
    pub eval: bool,
    /// `layers` returns whether the evaluation read the environment or the holder's state (otherwise always true).
    /// OFF by default (`RUSTCRAFT_OPT_ITEMENVFLAG=1` enables it): no code reads the
    /// flag yet and the wrappers cost about 4% per call (docs/perf.md).
    pub env_flag: bool,
    /// Quads of the models written in columns in the cache (read in bulk).
    pub quad_col: bool,
    /// Extruded layers kept as pixels, unfolded into quads on first request.
    pub quad_pack: bool,
}

impl Opt {
    pub fn get() -> Opt {
        #[cfg(test)]
        if let Some(o) = OPT_TEST.with(Cell::get) {
            return o;
        }
        static F: OnceLock<Opt> = OnceLock::new();
        *F.get_or_init(|| Opt {
            eval: rc_data::env_os!("RUSTCRAFT_OPT_ITEMEVAL").is_none_or(|v| v != "0"),
            env_flag: rc_data::env_os!("RUSTCRAFT_OPT_ITEMENVFLAG").is_some_and(|v| v == "1"),
            quad_col: rc_data::env_os!("RUSTCRAFT_OPT_ITEMQUADCOL").is_none_or(|v| v != "0"),
            quad_pack: rc_data::env_os!("RUSTCRAFT_OPT_ITEMQUADPACK").is_none_or(|v| v != "0"),
        })
    }

    /// Line of the baked-sections key: the shape written for item models depends on
    /// these two switches, so each setting has its own cache file.
    pub fn cache_tag(self) -> String {
        format!("items columns {} pixels {}\n", u8::from(self.quad_col), u8::from(self.quad_pack))
    }
}

#[cfg(test)]
thread_local! {
    /// Tests: switches forced on this thread.
    static OPT_TEST: Cell<Option<Opt>> = const { Cell::new(None) };
}

/// 4 × 4 matrix, row by row, applied to a column vector (`M · v`).
pub type Mat4 = [[f32; 4]; 4];

pub const IDENTITY: Mat4 = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

pub fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..4).map(|k| a[i][k] * b[k][j]).sum()))
}

pub fn translate(t: V3) -> Mat4 {
    let mut m = IDENTITY;
    m[0][3] = t[0];
    m[1][3] = t[1];
    m[2][3] = t[2];
    m
}

pub fn scale(s: V3) -> Mat4 {
    let mut m = IDENTITY;
    m[0][0] = s[0];
    m[1][1] = s[1];
    m[2][2] = s[2];
    m
}

/// Rotation of `a` radians around axis 0 (x), 1 (y) or 2 (z).
pub fn rotate(axis: usize, a: f32) -> Mat4 {
    let (s, c) = a.sin_cos();
    let mut m = IDENTITY;
    let (i, j) = match axis {
        0 => (1, 2),
        1 => (2, 0),
        _ => (0, 1),
    };
    m[i][i] = c;
    m[i][j] = -s;
    m[j][i] = s;
    m[j][j] = c;
    m
}

/// Quaternion as components: normalised by the format. The zero quaternion has no direction
/// (the game gets an undefined rotation and the element disappears): zero matrix, which squashes the
/// geometry to a point, hence invisible too.
fn quat(q: [f32; 4]) -> Mat4 {
    let n = (q.iter().map(|c| c * c).sum::<f32>()).sqrt();
    if n > 0.0 { quat_raw(q.map(|c| c / n)) } else { [[0.0; 4]; 4] }
}

/// Matrix of a quaternion as is, without normalisation: a quaternion of norm `k` gives the
/// rotation multiplied by `k²` (homogeneous form, w² + x² − y² − z² on the diagonal).
fn quat_raw([x, y, z, w]: [f32; 4]) -> Mat4 {
    let (xx, yy, zz, ww) = (x * x, y * y, z * z, w * w);
    [
        [ww + xx - yy - zz, 2.0 * (x * y - z * w), 2.0 * (x * z + y * w), 0.0],
        [2.0 * (x * y + z * w), ww - xx + yy - zz, 2.0 * (y * z - x * w), 0.0],
        [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), ww - xx - yy + zz, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

pub fn apply(m: &Mat4, v: V3) -> V3 {
    std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2] + m[i][3])
}

fn floats<const N: usize>(v: &Value) -> Option<[f32; N]> {
    let a = v.as_array().filter(|a| a.len() == N)?;
    let mut out = [0.0; N];
    for (o, x) in out.iter_mut().zip(a) {
        *o = x.as_f64()? as f32;
    }
    Some(out)
}

fn rotation(v: &Value) -> Option<Mat4> {
    if let Some(q) = floats::<4>(v) {
        return Some(quat(q));
    }
    let angle = v.get("angle")?.as_f64()? as f32;
    // Axis-angle form: the axis is NOT normalised (like the game); a non-unit axis rotates and
    // scales at the same time.
    let axis = floats::<3>(v.get("axis")?)?;
    let (s, c) = (angle / 2.0).sin_cos();
    Some(quat_raw([axis[0] * s, axis[1] * s, axis[2] * s, c]))
}

/// `transformation` field of a node: decomposed form or row-by-row matrix.
fn transformation(v: &Value) -> Option<Mat4> {
    if let Some(m) = floats::<16>(v) {
        return Some(std::array::from_fn(|i| std::array::from_fn(|j| m[i * 4 + j])));
    }
    let t = floats::<3>(v.get("translation")?)?;
    let l = rotation(v.get("left_rotation")?)?;
    let s = floats::<3>(v.get("scale")?)?;
    let r = rotation(v.get("right_rotation")?)?;
    Some(mat_mul(&mat_mul(&mat_mul(&translate(t), &l), &scale(s)), &r))
}

/// Tint source of a model (`items.md` part 1 § 4).
#[derive(Debug, Clone, PartialEq)]
pub enum Tint {
    Constant(u32),
    Dye(u32),
    /// Colour read from the grass map at load time.
    Grass(u32),
    Firework(u32),
    Potion(u32),
    Team(u32),
    CustomModelData {
        index: usize,
        default: u32,
    },
}

fn color(v: &Value) -> Option<u32> {
    if let Some(i) = v.as_i64() {
        return Some(i as u32);
    }
    let c = floats::<3>(v)?;
    Some(0xff00_0000 | c.iter().fold(0, |a, &f| a << 8 | ((f * 255.0) as u32 & 0xff)))
}

/// Boolean property (`condition`).
#[derive(Debug, Clone, PartialEq)]
pub enum CondProp {
    UsingItem,
    Broken,
    Damaged,
    HasComponent {
        component: String,
        ignore_default: bool,
    },
    FishingRodCast,
    BundleHasSelected,
    Selected,
    Carried,
    ExtendedView,
    KeybindDown(String),
    ViewEntity,
    CustomModelData(usize),
    /// Component predicate (`predicate` + `value`).
    Component(ComponentPred),
}

/// Component predicate of a `component` condition.
#[derive(Debug, Clone, PartialEq)]
pub enum ComponentPred {
    /// Damage: bounds (inclusive) of the remaining durability and of the damage.
    Damage { durability: [Option<i32>; 2], damage: [Option<i32>; 2] },
    /// Component type without its own predicate: presence of the component.
    Present(String),
    /// Own predicate not evaluated (enchantments, contents, books…): false.
    Unsupported,
}

/// Time zone of a `local_time` property: the system's, or a fixed offset (seconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeZone {
    System,
    Fixed(i32),
}

/// Last result of a `local_time` property and the instant of its next computation (ms of
/// the monotonic clock, [`EvalEnv::uptime_ms`]): the text is redone at most once per
/// second, like the game, even if the wall clock goes backwards. Run-time state, outside comparison.
#[derive(Debug, Clone)]
pub struct TimeCache(std::cell::RefCell<(i64, String)>);

impl Default for TimeCache {
    /// Never computed: the first call makes the text.
    fn default() -> Self {
        TimeCache(std::cell::RefCell::new((i64::MIN, String::new())))
    }
}

impl PartialEq for TimeCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SelectProp {
    CustomModelData(usize),
    MainHand,
    ChargeType,
    TrimMaterial,
    BlockState(String),
    DisplayContext,
    /// Date pattern (ICU-style letters), locale (`""`: root) and time zone.
    LocalTime { pattern: String, locale: String, zone: TimeZone, cache: TimeCache },
    ContextEntityType,
    ContextDimension,
    Component(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeSource {
    Daytime,
    MoonPhase,
    Random,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompassTarget {
    None,
    Lodestone,
    Spawn,
    Recovery,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NumProp {
    CustomModelData(usize),
    BundleFullness,
    Damage { normalize: bool },
    Cooldown,
    Time { wobble: bool, source: TimeSource },
    Compass { wobble: bool, target: CompassTarget },
    CrossbowPull,
    UseCycle(f32),
    UseDuration { remaining: bool },
    Count { normalize: bool },
}

/// Oscillator of the hands (clock, compass): state shared by the node, updated once
/// per tick (§ 5.1).
#[derive(Debug, Default)]
pub struct Wobbler {
    rot: Cell<f32>,
    delta: Cell<f32>,
    last_tick: Cell<i64>,
    started: Cell<bool>,
}

fn positive_mod(a: f32, b: f32) -> f32 {
    ((a % b) + b) % b
}

impl Wobbler {
    fn update(&self, tick: i64, target: f32, factor: f32) -> f32 {
        if !self.started.get() || self.last_tick.get() != tick {
            self.started.set(true);
            self.last_tick.set(tick);
            let d = positive_mod(target - self.rot.get() + 0.5, 1.0) - 0.5;
            self.delta.set((self.delta.get() + 0.1 * d) * factor);
            self.rot.set(positive_mod(self.rot.get() + self.delta.get(), 1.0));
        }
        self.rot.get()
    }
}

/// Node of an item definition; the transformations are already composed in the leaves.
#[derive(Debug)]
pub enum Node {
    Empty,
    Model {
        model: u32,
        tints: Vec<Tint>,
        local: Mat4,
    },
    Composite(Vec<Node>),
    Condition {
        prop: CondProp,
        on_true: Box<Node>,
        on_false: Box<Node>,
    },
    Select {
        prop: SelectProp,
        cases: Vec<(Vec<String>, Node)>,
        fallback: Box<Node>,
    },
    Range {
        prop: NumProp,
        scale: f32,
        entries: Vec<(f32, Node)>,
        fallback: Box<Node>,
        wobble: Wobbler,
        idle: Wobbler,
    },
    /// Special model: base model `base` and drawing of kind `kind`; non-empty `data` = data
    /// fixed by the definition (chest texture), instead of those of the stack.
    Special {
        base: u32,
        kind: String,
        local: Mat4,
        data: Vec<u32>,
    },
    BundleSelected,
    Missing {
        local: Mat4,
    },
}

/// What the evaluation reads on the stack.
pub trait StackView {
    /// The component is present (item default included when known).
    fn has(&self, component: &str) -> bool;
    /// Present through the stack's patch (different from the default).
    fn patched(&self, component: &str) -> bool {
        self.has(component)
    }
    fn damage(&self) -> i32 {
        0
    }
    fn max_damage(&self) -> i32 {
        0
    }
    fn count(&self) -> i32 {
        1
    }
    fn max_stack_size(&self) -> i32 {
        64
    }
    fn dyed_color(&self) -> Option<u32> {
        None
    }
    fn potion_color(&self) -> Option<u32> {
        None
    }
    /// Colours (RGB) of the stack's firework explosion, in order, passed one by one
    /// to `f` (nothing if the component is missing).
    fn firework_colors(&self, _f: &mut dyn FnMut(u32)) {}
    fn custom_model_data_flag(&self, _i: usize) -> Option<bool> {
        None
    }
    fn custom_model_data_float(&self, _i: usize) -> Option<f32> {
        None
    }
    fn custom_model_data_string(&self, _i: usize) -> Option<String> {
        None
    }
    fn custom_model_data_color(&self, _i: usize) -> Option<u32> {
        None
    }
    /// Data of a special model (`SPECIAL_KINDS`) drawn from the stack: head kind,
    /// banner colours and patterns… (empty: defaults).
    fn special_data(&self, _kind: &str) -> Vec<u32> {
        Vec::new()
    }
    fn block_state(&self, _property: &str) -> Option<String> {
        None
    }
    /// `none`, `arrow` or `rocket`.
    fn charge_type(&self) -> &'static str {
        "none"
    }
    /// Crossbow charge time in ticks: ⌊max(0, 1.25 s modified by the enchantments)
    /// × 20⌋ (25 without Quick Charge).
    fn crossbow_charge(&self) -> i32 {
        25
    }
    /// Material of the `trim` component (`minecraft:iron`…).
    fn trim_material(&self) -> Option<String> {
        None
    }
    /// Target of the `lodestone_tracker` component: dimension and block.
    fn lodestone(&self) -> Option<(String, [i32; 3])> {
        None
    }
    fn glint(&self) -> bool {
        false
    }
    /// Stack chosen in the local player's hotbar.
    fn selected(&self) -> bool {
        false
    }
    /// Stack being used by the holder: (ticks elapsed, ticks remaining).
    fn using(&self) -> Option<(i32, i32)> {
        None
    }
    /// Stack held in the hand that holds the line of an active fishing bobber of the holder (the
    /// main hand if it carries a fishing rod, otherwise the other).
    fn fishing_cast(&self) -> bool {
        false
    }
    /// Stack carried on the cursor of the local player's menu.
    fn carried(&self) -> bool {
        false
    }
    /// Item chosen from a bundle: `f(item, item model, stack)` called if there is one (true then).
    fn bundle_selected(&self, _f: &mut dyn FnMut(&str, &str, &dyn StackView)) -> bool {
        false
    }
}

/// What the evaluation reads on the world and the holder.
pub trait EvalEnv {
    /// A holder (local player) is present.
    fn has_holder(&self) -> bool {
        true
    }
    /// The holder is a living being: only the properties with a condition or a choice (hand,
    /// selection, entity type, camera view) see it; a dropped item or a frame is not
    /// one.
    fn living_holder(&self) -> bool {
        self.has_holder()
    }
    /// Identifier of the type of the living holder (`minecraft:zombie`…).
    fn holder_type(&self) -> Option<String> {
        self.living_holder().then(|| "minecraft:player".to_owned())
    }
    fn dimension(&self) -> Option<String> {
        None
    }
    fn tick(&self) -> i64 {
        0
    }
    /// Sun angle (degrees) and moon phase, at the holder.
    fn sun_angle(&self) -> f32 {
        0.0
    }
    fn moon_phase(&self) -> f32 {
        0.0
    }
    fn random(&self) -> f32 {
        0.0
    }
    /// Wall clock (ms since 1970, UTC); `None`: no clock (no time value).
    fn now_ms(&self) -> Option<i64> {
        None
    }
    /// Monotonic clock (ms since the process's first call): pacing of the `local_time`
    /// recomputation, insensitive to wall clock jumps backwards.
    fn uptime_ms(&self) -> i64 {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        START.get_or_init(std::time::Instant::now).elapsed().as_millis() as i64
    }
    /// Offset of the system time zone at instant `ms` (seconds, to add to UTC).
    fn utc_offset_secs(&self, _ms: i64) -> i64 {
        0
    }
    /// Last place of death of the holding player: dimension and block.
    fn last_death(&self) -> Option<(String, [i32; 3])> {
        None
    }
    /// The named binding (`key.use`…) is pressed (or toggled on).
    fn key_down(&self, _keybind: &str) -> bool {
        false
    }
    /// The holder is the camera entity (without a camera, the local player).
    fn view_entity(&self) -> bool {
        self.has_holder()
    }
    fn shift_down(&self) -> bool {
        false
    }
    /// Right main hand.
    fn right_handed(&self) -> bool {
        true
    }
    /// Display context (`gui`, `ground`, `thirdperson_righthand`…).
    fn display_context(&self) -> &str {
        "gui"
    }
    /// Position of the holder and rotation of its body (degrees).
    fn holder_pos(&self) -> [f64; 3] {
        [0.0; 3]
    }
    fn body_yaw(&self) -> f32 {
        0.0
    }
    /// Spawn point of the client world.
    fn spawn(&self) -> Option<(String, [i32; 3])> {
        None
    }
    /// The holder is the local player and the game runs at the normal rate.
    fn local_ticking(&self) -> bool {
        true
    }
    /// RGB colour of the holder's team, if the holder is in a coloured team.
    fn team_color(&self) -> Option<u32> {
        None
    }
}

/// Colours of a layer: up to four without heap, beyond that in a `Vec`.
#[derive(Clone)]
pub enum Tints {
    Inline(u8, [u32; 4]),
    Heap(Vec<u32>),
}

impl Tints {
    pub const fn new() -> Self {
        Tints::Inline(0, [0; 4])
    }
}

impl Default for Tints {
    fn default() -> Self {
        Tints::new()
    }
}

impl std::ops::Deref for Tints {
    type Target = [u32];
    fn deref(&self) -> &[u32] {
        match self {
            Tints::Inline(n, a) => &a[..usize::from(*n)],
            Tints::Heap(v) => v,
        }
    }
}

impl PartialEq for Tints {
    fn eq(&self, o: &Self) -> bool {
        **self == **o
    }
}

impl std::fmt::Debug for Tints {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl From<Vec<u32>> for Tints {
    fn from(v: Vec<u32>) -> Self {
        Tints::Heap(v)
    }
}

/// Layer produced by the evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub model: u32,
    pub tints: Tints,
    pub local: Mat4,
    /// Glint: 0 none, 1 standard, 2 special (compasses, clock).
    pub glint: u8,
    /// Special model drawn in addition to the base model: 0 none, otherwise index + 1 into
    /// [`SPECIAL_KINDS`]; `tints` then carries its data (`StackView::special_data`).
    pub special: u16,
}

/// Special models supported by the icons (`minecraft:special` types).
pub const SPECIAL_KINDS: [&str; 10] = [
    "minecraft:head",
    "minecraft:player_head",
    "minecraft:banner",
    "minecraft:shield",
    "minecraft:chest",
    "minecraft:shulker_box",
    "minecraft:conduit",
    "minecraft:trident",
    "minecraft:decorated_pot",
    "minecraft:copper_golem_statue",
];

/// Known chest textures (`texture` field of the `chest` special model, without namespace:
/// `entity/chest/<name>`); the index is the layer's data.
pub const CHEST_TEXTURES: [&str; 8] =
    ["normal", "trapped", "ender", "copper", "copper_exposed", "copper_weathered", "copper_oxidized", "christmas"];

/// Quad of a baked item model (model frame, 0..1).
#[derive(Debug, Clone, PartialEq)]
pub struct ItemQuad {
    pub pos: [V3; 4],
    /// UV as a fraction of the sprite.
    pub uv: [[f32; 2]; 4],
    pub sprite: u32,
    pub tint: i32,
    pub dir: Dir,
    pub translucent: bool,
}

/// Extruded layer kept as pixels (`RUSTCRAFT_OPT_ITEMQUADPACK`): front and back faces, then
/// one edge per entry of `edges` (`y << 17 | x << 2 | kind`, in (y, x, kind) order).
#[derive(Debug, Clone, PartialEq)]
struct PackedLayer {
    sprite: u32,
    layer: i32,
    translucent: bool,
    w: u32,
    h: u32,
    edges: Vec<u32>,
}

/// Maximum side of a sprite stored as pixels (15 bits per coordinate).
const PACK_MAX: u32 = 1 << 15;

#[derive(Debug, Clone)]
pub struct BakedItem {
    /// Quads, unfolded on first request when the model is stored as pixels (`packed`).
    quads: OnceLock<Vec<ItemQuad>>,
    packed: Vec<PackedLayer>,
    /// Shape of the quads in the cache outside pixel layers: 0 row by row, 1 in columns.
    form: u8,
    /// Face lighting (flat items) or block lighting.
    pub gui_front: bool,
    /// `display.gui` composed with the centring.
    pub display: Mat4,
    /// Transform of each context (`DISPLAY_CONTEXTS`), centring included.
    pub displays: [Mat4; crate::model::DISPLAY_CONTEXTS.len()],
    /// A sprite of the model is animated.
    pub animated: bool,
    /// "particle" sprite of the model (item break particles): `particle` texture, by default `layer0`
    /// for a flat model, otherwise the missing sprite.
    pub particle: u32,
}

impl BakedItem {
    /// Quads of the model (model frame), unfolded once if they are stored as pixels.
    pub fn quads(&self) -> &[ItemQuad] {
        self.quads.get_or_init(|| {
            let mut out = Vec::with_capacity(self.packed.iter().map(|l| 2 + l.edges.len()).sum());
            for l in &self.packed {
                unfold(l, &mut out);
            }
            out
        })
    }

    /// Bytes on the heap (unfolded quads and pixel layers).
    pub fn heap_bytes(&self) -> usize {
        self.quads.get().map_or(0, |q| q.capacity() * size_of::<ItemQuad>())
            + self.packed.iter().map(|l| size_of::<PackedLayer>() + l.edges.capacity() * 4).sum::<usize>()
    }
}

/// Generation of the item models: each load or cache re-read takes a new one
/// (to put in the keys of the caches that keep evaluations).
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_generation() -> u64 {
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// All the definitions and the baked item models.
pub struct ItemModels {
    defs: HashMap<String, (Node, bool)>,
    /// Hand swap animation (`hand_animation_on_swap`, `swap_animation_scale`), if
    /// different from the defaults (true, 1).
    swap: HashMap<String, (bool, f32)>,
    pub baked: Vec<BakedItem>,
    /// Sprites (first frame) and possible animations, by index of `ItemQuad::sprite`.
    pub sprites: Vec<Image>,
    pub sprite_names: Vec<String>,
    pub animations: Vec<Option<texture::Animation>>,
    /// Animation frames per sprite (empty if fixed).
    pub frames: Vec<Vec<Image>>,
    compasses: Vec<String>,
    /// Missing model (checkerboard cube), for the layers `model = u32::MAX`.
    pub missing: BakedItem,
    /// `textures/misc/enchanted_glint_item.png` (enchantment glint).
    pub glint: Option<Image>,
    generation: u64,
}

const MISSING_SPRITE: &str = "rustcraft:missing";

struct Loader<'a> {
    assets: &'a Assets,
    models: Models<'a>,
    model_index: HashMap<String, u32>,
    baked: Vec<BakedItem>,
    sprite_index: HashMap<String, u32>,
    sprites: Vec<Image>,
    sprite_names: Vec<String>,
    animations: Vec<Option<texture::Animation>>,
    frames: Vec<Vec<Image>>,
    grass: Option<Image>,
    /// `paletted_permutations` sources of the item atlas.
    perms: Vec<crate::palette::Permutations>,
}

fn norm(id: &str) -> String {
    let (ns, path) = split_id(id);
    format!("{ns}:{path}")
}

impl Loader<'_> {
    fn sprite(&mut self, name: &str) -> u32 {
        let key = norm(name);
        if let Some(&i) = self.sprite_index.get(&key) {
            return i;
        }
        // Palette permutations of the item atlas (trims): declared after the `item/`
        // directory, they win over a texture of the same name.
        let permuted = self.perms.iter().find_map(|p| p.resolve(self.assets, &key));
        let loaded = match permuted {
            Some(img) => Some(texture::Loaded { frames: vec![img], animation: None, levels: Vec::new() }),
            None => texture::load_animated(self.assets, &key),
        };
        let (img, anim, frames) = match loaded {
            Some(l) => {
                let first = l.animation.as_ref().and_then(|a| a.frames.first()).map_or(0, |f| f.0 as usize);
                let img = l.frames.get(first).cloned().unwrap_or_else(missing_image);
                let frames = if l.animation.is_some() { l.frames } else { Vec::new() };
                (img, l.animation, frames)
            }
            None => (missing_image(), None, Vec::new()),
        };
        let i = self.sprites.len() as u32;
        self.sprites.push(img);
        self.animations.push(anim);
        self.frames.push(frames);
        self.sprite_names.push(key.clone());
        self.sprite_index.insert(key, i);
        i
    }

    /// Bakes (once) the model `id`.
    fn model(&mut self, id: &str) -> u32 {
        let key = norm(id);
        if let Some(&i) = self.model_index.get(&key) {
            return i;
        }
        let baked = match self.models.flatten_item(&key) {
            Some(m) => self.bake(&m),
            None => self.bake_missing(),
        };
        let i = self.baked.len() as u32;
        self.baked.push(baked);
        self.model_index.insert(key, i);
        i
    }

    fn bake_missing(&mut self) -> BakedItem {
        let s = self.sprite(MISSING_SPRITE);
        let quads: Vec<ItemQuad> = Dir::ALL
            .iter()
            .map(|&d| {
                let c = crate::bake::CORNERS[d as usize];
                ItemQuad {
                    pos: c.map(|v| v.map(f32::from)),
                    uv: [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]],
                    sprite: s,
                    tint: -1,
                    dir: d,
                    translucent: false,
                }
            })
            .collect();
        BakedItem {
            quads: OnceLock::from(quads),
            packed: Vec::new(),
            form: u8::from(Opt::get().quad_col),
            gui_front: false,
            display: display_matrix(None),
            displays: [display_matrix(None); crate::model::DISPLAY_CONTEXTS.len()],
            animated: false,
            particle: s,
        }
    }

    fn bake(&mut self, m: &ItemModel) -> BakedItem {
        let opt = Opt::get();
        let mut quads = Vec::new();
        let mut packed = Vec::new();
        match &m.geometry {
            ItemGeometry::Generated if opt.quad_pack => {
                let mut todo = Vec::new();
                for layer in 0..5 {
                    let Some(t) = m.resolve(&format!("layer{layer}")) else { break };
                    todo.push((self.sprite(&t.value), layer, t.force_translucent));
                }
                // Sprite too large for 15 bits per coordinate: quads right away.
                let fits = todo.iter().all(|&(s, ..)| {
                    let img = &self.sprites[s as usize];
                    img.width <= PACK_MAX && img.height <= PACK_MAX
                });
                for (s, layer, force) in todo {
                    let (img, frames) = (&self.sprites[s as usize], &self.frames[s as usize]);
                    if fits {
                        packed.push(pack_layer(img, frames, s, layer, force));
                    } else {
                        extrude(img, frames, s, layer, force, &mut quads);
                    }
                }
            }
            ItemGeometry::Generated => {
                for layer in 0..5 {
                    let Some(t) = m.resolve(&format!("layer{layer}")) else { break };
                    let s = self.sprite(&t.value);
                    extrude(
                        &self.sprites[s as usize],
                        &self.frames[s as usize],
                        s,
                        layer,
                        t.force_translucent,
                        &mut quads,
                    );
                }
            }
            ItemGeometry::Elements(elements) => {
                // Faces without inset first (elements × faces), then those with an inset per direction.
                let mut culled: Vec<(Dir, ItemQuad)> = Vec::new();
                for e in elements.iter() {
                    for d in Dir::ALL {
                        let Some(face) = &e.faces[d as usize] else { continue };
                        let tex = m.resolve(&face.texture);
                        let (name, force) =
                            tex.map_or_else(|| (MISSING_SPRITE.to_owned(), false), |t| (t.value, t.force_translucent));
                        let s = self.sprite(&name);
                        let uvs = face.uv.unwrap_or_else(|| crate::bake::default_uv(e.from, e.to, d));
                        let translucent = force || {
                            let img = &self.sprites[s as usize];
                            let frames = &self.frames[s as usize];
                            let anim = self.animations[s as usize].as_ref().map(|a| (frames.as_slice(), a));
                            texture::sprite_transparency(
                                img,
                                anim,
                                uvs[0].min(uvs[2]) / 16.0,
                                uvs[1].min(uvs[3]) / 16.0,
                                uvs[0].max(uvs[2]) / 16.0,
                                uvs[1].max(uvs[3]) / 16.0,
                            )
                            .translucent
                        };
                        let mut pos = [[0.0f32; 3]; 4];
                        let mut uv = [[0.0f32; 2]; 4];
                        for i in 0..4 {
                            let c = crate::bake::CORNERS[d as usize][i];
                            let mut p: V3 = [0, 1, 2].map(|a| if c[a] == 1 { e.to[a] } else { e.from[a] } / 16.0);
                            if let Some(r) = &e.rotation {
                                let q = crate::geom::mul_v(
                                    &r.matrix,
                                    [p[0] - r.origin[0], p[1] - r.origin[1], p[2] - r.origin[2]],
                                );
                                p = [q[0] + r.origin[0], q[1] + r.origin[1], q[2] + r.origin[2]];
                            }
                            pos[i] = p;
                            let j = (i + face.rotation as usize) % 4;
                            uv[i] = [
                                if j == 0 || j == 1 { uvs[0] } else { uvs[2] } / 16.0,
                                if j == 0 || j == 3 { uvs[1] } else { uvs[3] } / 16.0,
                            ];
                        }
                        let a = sub(pos[1], pos[0]);
                        let b = sub(pos[2], pos[0]);
                        let n = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
                        let dir = Dir::nearest(n).unwrap_or(d);
                        let q = ItemQuad { pos, uv, sprite: s, tint: face.tint, dir, translucent };
                        match face.cull {
                            Some(c) => culled.push((c, q)),
                            None => quads.push(q),
                        }
                    }
                }
                for d in [Dir::North, Dir::South, Dir::East, Dir::West, Dir::Up, Dir::Down] {
                    quads.extend(culled.iter().filter(|(c, _)| *c == d).map(|(_, q)| q.clone()));
                }
            }
        }
        let animated = quads.iter().any(|q| self.animations[q.sprite as usize].is_some())
            || packed.iter().any(|l| self.animations[l.sprite as usize].is_some());
        let quads = if packed.is_empty() { OnceLock::from(quads) } else { OnceLock::new() };
        // Flat model: `particle` refers to `layer0` by default.
        let particle = match &m.geometry {
            ItemGeometry::Generated => m.resolve("particle").or_else(|| m.resolve("layer0")),
            ItemGeometry::Elements(_) => m.resolve("particle"),
        };
        let particle = self.sprite(&particle.map_or_else(|| MISSING_SPRITE.to_owned(), |t| t.value));
        BakedItem {
            quads,
            packed,
            form: u8::from(opt.quad_col),
            gui_front: m.gui_front,
            display: display_matrix(m.display_gui.as_ref()),
            displays: m.display.map(|d| display_matrix(d.as_ref())),
            animated,
            particle,
        }
    }

    /// Definition node; `parent` = composed transformation of the parent.
    fn node(&mut self, v: &Value, parent: &Mat4) -> Node {
        let local = match v.get("transformation").and_then(transformation) {
            Some(m) => mat_mul(parent, &m),
            None => *parent,
        };
        let ty = v.get("type").and_then(Value::as_str).map(norm).unwrap_or_default();
        let sub = |l: &mut Self, k: &str| match v.get(k) {
            Some(n) => l.node(n, &local),
            None => Node::Missing { local },
        };
        match ty.as_str() {
            "minecraft:empty" => Node::Empty,
            "minecraft:model" => {
                let Some(id) = v.get("model").and_then(Value::as_str) else { return Node::Missing { local } };
                let model = self.model(id);
                let tints = v
                    .get("tints")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().map(|t| self.tint(t)).collect())
                    .unwrap_or_default();
                Node::Model { model, tints, local }
            }
            "minecraft:composite" => {
                let list: Vec<Node> = v
                    .get("models")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().map(|n| self.node(n, &local)).collect())
                    .unwrap_or_default();
                match list.len() {
                    0 => Node::Empty,
                    1 => list.into_iter().next().expect("one element"),
                    _ => Node::Composite(list),
                }
            }
            "minecraft:condition" => {
                let prop = cond_prop(v);
                let on_true = Box::new(sub(self, "on_true"));
                let on_false = Box::new(sub(self, "on_false"));
                Node::Condition { prop, on_true, on_false }
            }
            "minecraft:select" => {
                let prop = select_prop(v);
                let cases = v
                    .get("cases")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .map(|c| {
                                let when = match c.get("when") {
                                    Some(Value::Array(l)) => l.iter().map(when_value).collect(),
                                    Some(w) => vec![when_value(w)],
                                    None => Vec::new(),
                                };
                                let model = match c.get("model") {
                                    Some(m) => self.node(m, &local),
                                    None => Node::Missing { local },
                                };
                                (when, model)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Node::Select { prop, cases, fallback: Box::new(sub(self, "fallback")) }
            }
            "minecraft:range_dispatch" => {
                let prop = num_prop(v);
                let scale = v.get("scale").and_then(Value::as_f64).unwrap_or(1.0) as f32;
                let mut entries: Vec<(f32, Node)> = v
                    .get("entries")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .map(|e| {
                                let t = e.get("threshold").and_then(Value::as_f64).unwrap_or(0.0) as f32;
                                let model = match e.get("model") {
                                    Some(m) => self.node(m, &local),
                                    None => Node::Missing { local },
                                };
                                (t, model)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                entries.sort_by(|a, b| a.0.total_cmp(&b.0));
                Node::Range {
                    prop,
                    scale,
                    entries,
                    fallback: Box::new(sub(self, "fallback")),
                    wobble: Wobbler::default(),
                    idle: Wobbler::default(),
                }
            }
            "minecraft:special" => {
                let base_id = v.get("base").and_then(Value::as_str).unwrap_or(MISSING_SPRITE).to_owned();
                let base = self.model(&base_id);
                let kind = v.pointer("/model/type").and_then(Value::as_str).map(norm).unwrap_or_default();
                // Chest: texture forced by the definition (unknown texture: the normal one).
                let data = match (kind.as_str(), v.pointer("/model/texture").and_then(Value::as_str)) {
                    ("minecraft:chest", Some(t)) => {
                        let t = norm(t);
                        let name = t.strip_prefix("minecraft:").unwrap_or(&t);
                        vec![CHEST_TEXTURES.iter().position(|c| *c == name).unwrap_or(0) as u32]
                    }
                    _ => Vec::new(),
                };
                Node::Special { base, kind, local, data }
            }
            "minecraft:bundle/selected_item" => Node::BundleSelected,
            _ => Node::Missing { local },
        }
    }

    fn tint(&mut self, t: &Value) -> Tint {
        let ty = t.get("type").and_then(Value::as_str).map(norm).unwrap_or_default();
        let def = |k: &str| t.get(k).and_then(color).unwrap_or(0xffff_ffff);
        match ty.as_str() {
            "minecraft:constant" => Tint::Constant(def("value") | 0xff00_0000),
            "minecraft:dye" => Tint::Dye(def("default")),
            "minecraft:grass" => {
                let temp = t.get("temperature").and_then(Value::as_f64).unwrap_or(0.5) as f32;
                let rain = t.get("downfall").and_then(Value::as_f64).unwrap_or(1.0) as f32 * temp;
                let x = ((1.0 - temp) * 255.0) as u32;
                let y = ((1.0 - rain) * 255.0) as u32;
                let c = self.grass.as_ref().filter(|g| x < g.width && y < g.height).map_or(0xffff_00ff, |g| {
                    let i = ((y * g.width + x) * 4) as usize;
                    let p = &g.rgba[i..i + 4];
                    u32::from(p[3]) << 24 | u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2])
                });
                Tint::Grass(c)
            }
            "minecraft:firework" => Tint::Firework(def("default")),
            "minecraft:potion" => Tint::Potion(def("default") | 0xff00_0000),
            "minecraft:team" => Tint::Team(def("default") | 0xff00_0000),
            "minecraft:custom_model_data" => Tint::CustomModelData {
                index: t.get("index").and_then(Value::as_u64).unwrap_or(0) as usize,
                default: def("default") | 0xff00_0000,
            },
            _ => Tint::Constant(0xffff_ffff),
        }
    }
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn missing_image() -> Image {
    Image::missing(16)
}

/// `T(t) · Rx · Ry · Rz · S · T(−½)`; without `display.gui`: centring only.
fn display_matrix(d: Option<&crate::model::Display>) -> Mat4 {
    let center = translate([-0.5; 3]);
    let Some(d) = d else { return center };
    let r = |a: usize| rotate(a, d.rotation[a].to_radians());
    let rot = mat_mul(&mat_mul(&r(0), &r(1)), &r(2));
    mat_mul(&mat_mul(&mat_mul(&translate(d.translation), &rot), &scale(d.scale)), &center)
}

fn when_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn prop_name(v: &Value) -> String {
    v.get("property").and_then(Value::as_str).map(norm).unwrap_or_default()
}

fn index(v: &Value) -> usize {
    v.get("index").and_then(Value::as_u64).unwrap_or(0) as usize
}

fn cond_prop(v: &Value) -> CondProp {
    match prop_name(v).as_str() {
        "minecraft:using_item" => CondProp::UsingItem,
        "minecraft:broken" => CondProp::Broken,
        "minecraft:damaged" => CondProp::Damaged,
        "minecraft:has_component" => CondProp::HasComponent {
            component: v.get("component").and_then(Value::as_str).map(norm).unwrap_or_default(),
            ignore_default: v.get("ignore_default").and_then(Value::as_bool).unwrap_or(false),
        },
        "minecraft:fishing_rod/cast" => CondProp::FishingRodCast,
        "minecraft:bundle/has_selected_item" => CondProp::BundleHasSelected,
        "minecraft:selected" => CondProp::Selected,
        "minecraft:carried" => CondProp::Carried,
        "minecraft:extended_view" => CondProp::ExtendedView,
        "minecraft:keybind_down" => {
            CondProp::KeybindDown(v.get("keybind").and_then(Value::as_str).unwrap_or_default().to_owned())
        }
        "minecraft:view_entity" => CondProp::ViewEntity,
        "minecraft:custom_model_data" => CondProp::CustomModelData(index(v)),
        _ => CondProp::Component(component_pred(v)),
    }
}

/// Integer bounds `n` or `{min, max}` (absent: any).
fn int_bounds(v: Option<&Value>) -> [Option<i32>; 2] {
    let int = |v: Option<&Value>| v.and_then(Value::as_i64).map(|n| n as i32);
    match v {
        Some(Value::Number(_)) => [int(v), int(v)],
        Some(o) => [int(o.get("min")), int(o.get("max"))],
        None => [None, None],
    }
}

/// Predicates with their own value shape (not evaluated except damage); any other
/// component type tests presence.
const OWN_PREDICATES: [&str; 14] = [
    "enchantments",
    "stored_enchantments",
    "potion_contents",
    "custom_data",
    "container",
    "bundle_contents",
    "firework_explosion",
    "fireworks",
    "writable_book_content",
    "written_book_content",
    "attribute_modifiers",
    "trim",
    "jukebox_playable",
    "villager/variant",
];

fn component_pred(v: &Value) -> ComponentPred {
    let Some(p) = v.get("predicate").and_then(Value::as_str).map(norm) else { return ComponentPred::Unsupported };
    let value = v.get("value");
    match p.strip_prefix("minecraft:").unwrap_or(&p) {
        "damage" => ComponentPred::Damage {
            durability: int_bounds(value.and_then(|o| o.get("durability"))),
            damage: int_bounds(value.and_then(|o| o.get("damage"))),
        },
        name if OWN_PREDICATES.contains(&name) => ComponentPred::Unsupported,
        _ => ComponentPred::Present(p),
    }
}

/// Time zone of a `local_time` property: fixed-offset identifiers (`UTC`, `GMT`,
/// `GMT+1`, `GMT-05:30`, `Etc/GMT+3`…); any other zone name takes the system's.
fn time_zone(id: Option<&str>) -> TimeZone {
    let Some(id) = id else { return TimeZone::System };
    if matches!(id, "UTC" | "GMT" | "UCT" | "Etc/UTC" | "Etc/GMT" | "Etc/UCT" | "Zulu" | "Universal" | "Greenwich") {
        return TimeZone::Fixed(0);
    }
    // `Etc/GMT±n` zones: inverted sign (POSIX convention).
    if let Some(n) = id.strip_prefix("Etc/GMT").and_then(|n| n.parse::<i32>().ok()) {
        return TimeZone::Fixed(-n * 3600);
    }
    // Free form `GMT` sign hours [`:`] [minutes].
    let Some(rest) = id.strip_prefix("GMT") else { return TimeZone::System };
    let (sign, digits) = match rest.as_bytes().first() {
        Some(b'+') => (1, &rest[1..]),
        Some(b'-') => (-1, &rest[1..]),
        _ => return TimeZone::System,
    };
    let (h, m) = match digits.split_once(':') {
        Some((h, m)) if (1..=2).contains(&h.len()) && m.len() == 2 => (h, m),
        None if (1..=2).contains(&digits.len()) => (digits, "0"),
        None if digits.len() == 4 => (&digits[..2], &digits[2..]),
        _ => return TimeZone::System,
    };
    match (h.parse::<i32>(), m.parse::<i32>()) {
        (Ok(h), Ok(m)) if h <= 23 && m <= 59 => TimeZone::Fixed(sign * (h * 3600 + m * 60)),
        _ => TimeZone::System,
    }
}

fn select_prop(v: &Value) -> SelectProp {
    match prop_name(v).as_str() {
        "minecraft:custom_model_data" => SelectProp::CustomModelData(index(v)),
        "minecraft:main_hand" => SelectProp::MainHand,
        "minecraft:charge_type" => SelectProp::ChargeType,
        "minecraft:trim_material" => SelectProp::TrimMaterial,
        "minecraft:block_state" => {
            SelectProp::BlockState(v.get("block_state_property").and_then(Value::as_str).unwrap_or_default().to_owned())
        }
        "minecraft:display_context" => SelectProp::DisplayContext,
        "minecraft:local_time" => {
            let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
            SelectProp::LocalTime {
                pattern: s("pattern"),
                locale: s("locale"),
                zone: time_zone(v.get("time_zone").and_then(Value::as_str)),
                cache: TimeCache::default(),
            }
        }
        "minecraft:context_entity_type" => SelectProp::ContextEntityType,
        "minecraft:context_dimension" => SelectProp::ContextDimension,
        other => SelectProp::Component(other.to_owned()),
    }
}

fn num_prop(v: &Value) -> NumProp {
    let flag = |k: &str, d: bool| v.get(k).and_then(Value::as_bool).unwrap_or(d);
    match prop_name(v).as_str() {
        "minecraft:custom_model_data" => NumProp::CustomModelData(index(v)),
        "minecraft:bundle/fullness" => NumProp::BundleFullness,
        "minecraft:damage" => NumProp::Damage { normalize: flag("normalize", true) },
        "minecraft:cooldown" => NumProp::Cooldown,
        "minecraft:time" => NumProp::Time {
            wobble: flag("wobble", true),
            source: match v.get("source").and_then(Value::as_str) {
                Some("moon_phase") => TimeSource::MoonPhase,
                Some("random") => TimeSource::Random,
                _ => TimeSource::Daytime,
            },
        },
        "minecraft:compass" => NumProp::Compass {
            wobble: flag("wobble", true),
            target: match v.get("target").and_then(Value::as_str) {
                Some("lodestone") => CompassTarget::Lodestone,
                Some("spawn") => CompassTarget::Spawn,
                Some("recovery") => CompassTarget::Recovery,
                _ => CompassTarget::None,
            },
        },
        "minecraft:crossbow/pull" => NumProp::CrossbowPull,
        "minecraft:use_cycle" => NumProp::UseCycle(v.get("period").and_then(Value::as_f64).unwrap_or(1.0) as f32),
        "minecraft:use_duration" => NumProp::UseDuration { remaining: flag("remaining", false) },
        "minecraft:count" => NumProp::Count { normalize: flag("normalize", true) },
        _ => NumProp::CustomModelData(usize::MAX),
    }
}

/// Extrusion of a layer (`items.md` part 2 § 2): south and north faces, then one edge per side
/// of an opaque pixel next to a transparent pixel (union over the frames of an animation).
fn extrude(img: &Image, frames: &[Image], sprite: u32, layer: i32, force: bool, out: &mut Vec<ItemQuad>) {
    // Whole texture: all the frames of an animation count, like the game.
    let translucent = force
        || img.transparency(0.0, 0.0, 1.0, 1.0).translucent
        || frames.iter().any(|f| f.transparency(0.0, 0.0, 1.0, 1.0).translucent);
    let (z0, z1) = (7.5 / 16.0, 8.5 / 16.0);
    let quad = |pos: [V3; 4], uv: [[f32; 2]; 4], dir: Dir| ItemQuad { pos, uv, sprite, tint: layer, dir, translucent };
    out.push(quad(
        [[0.0, 1.0, z1], [0.0, 0.0, z1], [1.0, 0.0, z1], [1.0, 1.0, z1]],
        [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]],
        Dir::South,
    ));
    out.push(quad(
        [[1.0, 1.0, z0], [1.0, 0.0, z0], [0.0, 0.0, z0], [0.0, 1.0, z0]],
        [[1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [0.0, 0.0]],
        Dir::North,
    ));
    let (w, h) = (img.width as i32, img.height as i32);
    let list: Vec<&Image> = if frames.is_empty() { vec![img] } else { frames.iter().collect() };
    // Edges: (kind 0 top, 1 bottom, 2 left, 3 right), stored for a fixed order.
    let mut edges = std::collections::BTreeSet::new();
    for f in &list {
        let alpha = |x: i32, y: i32| {
            if x < 0 || y < 0 || x >= w || y >= h || x >= f.width as i32 || y >= f.height as i32 {
                0
            } else {
                f.rgba[((y * f.width as i32 + x) * 4 + 3) as usize]
            }
        };
        for y in 0..h {
            for x in 0..w {
                if alpha(x, y) == 0 {
                    continue;
                }
                for (k, (dx, dy)) in [(0, -1), (0, 1), (-1, 0), (1, 0)].into_iter().enumerate() {
                    if alpha(x + dx, y + dy) == 0 {
                        edges.insert((y, x, k));
                    }
                }
            }
        }
    }
    let (sx, sy) = (1.0 / w as f32, 1.0 / h as f32);
    for (y, x, k) in edges {
        let (x0, x1) = (x as f32 * sx, (x + 1) as f32 * sx);
        let (yh, yb) = (1.0 - y as f32 * sy, 1.0 - (y + 1) as f32 * sy);
        let (u0, u1) = ((x as f32 + 0.1) * sx, (x as f32 + 0.9) * sx);
        let (va, vb) = ((y as f32 + 0.1) * sy, (y as f32 + 0.9) * sy);
        let q = match k {
            0 => quad(
                [[x0, yh, z0], [x0, yh, z1], [x1, yh, z1], [x1, yh, z0]],
                [[u0, va], [u0, vb], [u1, vb], [u1, va]],
                Dir::Up,
            ),
            1 => quad(
                [[x0, yb, z1], [x0, yb, z0], [x1, yb, z0], [x1, yb, z1]],
                [[u0, va], [u0, vb], [u1, vb], [u1, va]],
                Dir::Down,
            ),
            2 => quad(
                [[x0, yh, z0], [x0, yb, z0], [x0, yb, z1], [x0, yh, z1]],
                [[u0, vb], [u0, va], [u1, va], [u1, vb]],
                Dir::West,
            ),
            _ => quad(
                [[x1, yh, z1], [x1, yb, z1], [x1, yb, z0], [x1, yh, z0]],
                [[u0, vb], [u0, va], [u1, va], [u1, vb]],
                Dir::East,
            ),
        };
        out.push(q);
    }
}

/// Extruded layer stored as pixels: same transparency and edges as `extrude`, without the quads.
fn pack_layer(img: &Image, frames: &[Image], sprite: u32, layer: i32, force: bool) -> PackedLayer {
    let translucent = force
        || img.transparency(0.0, 0.0, 1.0, 1.0).translucent
        || frames.iter().any(|f| f.transparency(0.0, 0.0, 1.0, 1.0).translucent);
    let (w, h) = (img.width as i32, img.height as i32);
    let mut edges = Vec::new();
    let list: &[Image] = if frames.is_empty() { std::slice::from_ref(img) } else { frames };
    for f in list {
        let (fw, fh) = (w.min(f.width as i32), h.min(f.height as i32));
        let alpha = |x: i32, y: i32| {
            if x < 0 || y < 0 || x >= fw || y >= fh { 0 } else { f.rgba[((y * f.width as i32 + x) * 4 + 3) as usize] }
        };
        for y in 0..h {
            for x in 0..w {
                if alpha(x, y) == 0 {
                    continue;
                }
                for (k, (dx, dy)) in [(0, -1), (0, 1), (-1, 0), (1, 0)].into_iter().enumerate() {
                    if alpha(x + dx, y + dy) == 0 {
                        edges.push((y as u32) << 17 | (x as u32) << 2 | k as u32);
                    }
                }
            }
        }
    }
    // Order (y, x, kind) and union over the frames, like the ordered set of `extrude`.
    edges.sort_unstable();
    edges.dedup();
    PackedLayer { sprite, layer, translucent, w: img.width, h: img.height, edges }
}

/// Quads of a layer stored as pixels, in the order and with the numbers of `extrude`.
fn unfold(l: &PackedLayer, out: &mut Vec<ItemQuad>) {
    let (sprite, tint, translucent) = (l.sprite, l.layer, l.translucent);
    let (z0, z1) = (7.5 / 16.0, 8.5 / 16.0);
    let quad = |pos: [V3; 4], uv: [[f32; 2]; 4], dir: Dir| ItemQuad { pos, uv, sprite, tint, dir, translucent };
    out.push(quad(
        [[0.0, 1.0, z1], [0.0, 0.0, z1], [1.0, 0.0, z1], [1.0, 1.0, z1]],
        [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]],
        Dir::South,
    ));
    out.push(quad(
        [[1.0, 1.0, z0], [1.0, 0.0, z0], [0.0, 0.0, z0], [0.0, 1.0, z0]],
        [[1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [0.0, 0.0]],
        Dir::North,
    ));
    let (w, h) = (l.w as i32, l.h as i32);
    let (sx, sy) = (1.0 / w as f32, 1.0 / h as f32);
    for &e in &l.edges {
        let (y, x) = ((e >> 17) as i32, (e >> 2 & 0x7fff) as i32);
        let (x0, x1) = (x as f32 * sx, (x + 1) as f32 * sx);
        let (yh, yb) = (1.0 - y as f32 * sy, 1.0 - (y + 1) as f32 * sy);
        let (u0, u1) = ((x as f32 + 0.1) * sx, (x as f32 + 0.9) * sx);
        let (va, vb) = ((y as f32 + 0.1) * sy, (y as f32 + 0.9) * sy);
        out.push(match e & 3 {
            0 => quad(
                [[x0, yh, z0], [x0, yh, z1], [x1, yh, z1], [x1, yh, z0]],
                [[u0, va], [u0, vb], [u1, vb], [u1, va]],
                Dir::Up,
            ),
            1 => quad(
                [[x0, yb, z1], [x0, yb, z0], [x1, yb, z0], [x1, yb, z1]],
                [[u0, va], [u0, vb], [u1, vb], [u1, va]],
                Dir::Down,
            ),
            2 => quad(
                [[x0, yh, z0], [x0, yb, z0], [x0, yb, z1], [x0, yh, z1]],
                [[u0, vb], [u0, va], [u1, va], [u1, vb]],
                Dir::West,
            ),
            _ => quad(
                [[x1, yh, z1], [x1, yb, z1], [x1, yb, z0], [x1, yh, z0]],
                [[u0, vb], [u0, va], [u1, va], [u1, vb]],
                Dir::East,
            ),
        });
    }
}

impl ItemModels {
    /// Loads the definitions of `models` (`item_model` identifiers, e.g. `minecraft:stone`).
    pub fn load<'a>(assets: &Assets, ids: impl IntoIterator<Item = &'a str>) -> Self {
        let mut l = Loader {
            assets,
            models: Models::new(assets),
            model_index: HashMap::new(),
            baked: Vec::new(),
            sprite_index: HashMap::new(),
            sprites: Vec::new(),
            sprite_names: Vec::new(),
            animations: Vec::new(),
            frames: Vec::new(),
            grass: texture::load(assets, "minecraft:colormap/grass"),
            perms: crate::palette::Permutations::of_atlas(assets, "items"),
        };
        let mut defs = HashMap::new();
        let mut swap = HashMap::new();
        for id in ids {
            let key = norm(id);
            if key.is_empty() || defs.contains_key(&key) {
                continue;
            }
            let (ns, path) = split_id(&key);
            let Some(json) = assets
                .read(&format!("assets/{ns}/items/{path}.json"))
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            else {
                continue;
            };
            let Some(model) = json.get("model") else { continue };
            let node = l.node(model, &IDENTITY);
            let oversized = json.get("oversized_in_gui").and_then(Value::as_bool).unwrap_or(false);
            let on_swap = json.get("hand_animation_on_swap").and_then(Value::as_bool).unwrap_or(true);
            let scale = json.get("swap_animation_scale").and_then(Value::as_f64).unwrap_or(1.0) as f32;
            if !on_swap || scale != 1.0 {
                swap.insert(key.clone(), (on_swap, scale));
            }
            defs.insert(key, (node, oversized));
        }
        let missing = l.bake_missing();
        let compasses = ["minecraft:compass", "minecraft:recovery_compass"].map(str::to_owned).to_vec();
        ItemModels {
            defs,
            swap,
            baked: l.baked,
            sprites: l.sprites,
            sprite_names: l.sprite_names,
            animations: l.animations,
            frames: l.frames,
            compasses,
            missing,
            glint: texture::load(assets, "minecraft:misc/enchanted_glint_item"),
            generation: next_generation(),
        }
    }

    /// Generation of these models: different for each load or cache re-read.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    /// Hand swap animation of an `item_model`: (replayed on swap, height scale).
    pub fn swap_animation(&self, model_id: &str) -> (bool, f32) {
        let found = if Opt::get().eval {
            with_norm(model_id, |k| self.swap.get(k).copied())
        } else {
            self.swap.get(&norm(model_id)).copied()
        };
        found.unwrap_or((true, 1.0))
    }

    /// Layers of a stack in a GUI slot (`item_model` = `model_id`, item `item`); `seed`
    /// = slot seed. `key` receives the render identity (atlas cache key).
    ///
    /// Returns false when the evaluation read neither the environment nor the holder's state (use, selected
    /// slot): same layers for the same stack as long as `generation` does not change. True otherwise,
    /// and always true without `RUSTCRAFT_OPT_ITEMENVFLAG=1` (default).
    #[allow(clippy::too_many_arguments)]
    pub fn layers(
        &self,
        item: &str,
        model_id: &str,
        stack: &dyn StackView,
        env: &dyn EvalEnv,
        seed: i32,
        out: &mut Vec<Layer>,
        key: &mut Vec<u64>,
    ) -> bool {
        let opt = Opt::get();
        if !opt.env_flag {
            self.layers_with(opt.eval, item, model_id, stack, env, seed, out, key);
            return true;
        }
        let read = Cell::new(false);
        let (stack, env) = (WatchStack { s: stack, read: &read }, WatchEnv { e: env, read: &read });
        self.layers_with(opt.eval, item, model_id, &stack, &env, seed, out, key);
        read.get()
    }

    #[allow(clippy::too_many_arguments)]
    fn layers_with(
        &self,
        fast: bool,
        item: &str,
        model_id: &str,
        stack: &dyn StackView,
        env: &dyn EvalEnv,
        seed: i32,
        out: &mut Vec<Layer>,
        key: &mut Vec<u64>,
    ) {
        let glint = if !stack.glint() {
            0
        } else if self.compasses.iter().any(|c| c == item) || item == "minecraft:clock" {
            2
        } else {
            1
        };
        let def = if fast {
            with_norm(model_id, |k| self.defs.get(k))
        } else if model_id.contains(':') {
            // Identifier already complete (common case): no new string.
            self.defs.get(model_id)
        } else {
            self.defs.get(&norm(model_id))
        };
        match def {
            Some((node, _)) if fast => self.eval_new(node, stack, env, seed, glint, out, key),
            Some((node, _)) => self.eval_old(node, stack, env, seed, glint, out, key),
            None => {
                key.push(u64::MAX);
                out.push(Layer { model: u32::MAX, tints: Tints::new(), local: IDENTITY, glint: 0, special: 0 });
            }
        }
    }

    /// Evaluation without its own allocation (`RUSTCRAFT_OPT_ITEMEVAL`): same layers and same key
    /// as `eval_old`.
    #[allow(clippy::too_many_arguments)]
    fn eval_new(
        &self,
        node: &Node,
        stack: &dyn StackView,
        env: &dyn EvalEnv,
        seed: i32,
        glint: u8,
        out: &mut Vec<Layer>,
        key: &mut Vec<u64>,
    ) {
        key.push(node as *const Node as u64);
        match node {
            Node::Empty => {}
            // Item chosen from the bundle: its layers, evaluated like a separate stack.
            Node::BundleSelected => {
                stack.bundle_selected(&mut |item, model, inner| {
                    self.layers_with(true, item, model, inner, env, seed, out, key)
                });
            }
            Node::Model { model, tints, local } => {
                let tints = if tints.len() <= 4 {
                    let mut a = [0; 4];
                    for (c, t) in a.iter_mut().zip(tints) {
                        *c = tint_color(t, stack, env);
                    }
                    Tints::Inline(tints.len() as u8, a)
                } else {
                    Tints::Heap(tints.iter().map(|t| tint_color(t, stack, env)).collect())
                };
                key.push(u64::from(glint));
                key.extend(tints.iter().map(|&t| u64::from(t)));
                out.push(Layer { model: *model, tints, local: *local, glint, special: 0 });
            }
            Node::Composite(list) => {
                for n in list {
                    self.eval_new(n, stack, env, seed, glint, out, key);
                }
            }
            Node::Condition { prop, on_true, on_false } => {
                let v = cond(prop, stack, env);
                self.eval_new(if v { on_true } else { on_false }, stack, env, seed, glint, out, key);
            }
            Node::Select { prop, cases, fallback } => {
                let pick = |v: Option<&str>| {
                    v.and_then(|v| cases.iter().find(|(w, _)| w.iter().any(|x| norm_eq_fast(x, v))).map(|(_, n)| n))
                        .unwrap_or(fallback)
                };
                // Local time: text kept in the node, compared without copy.
                let next = if matches!(prop, SelectProp::LocalTime { .. }) {
                    local_time::with_value(prop, env, pick)
                } else {
                    pick(select_cow(prop, stack, env).as_deref())
                };
                self.eval_new(next, stack, env, seed, glint, out, key);
            }
            Node::Range { prop, scale, entries, fallback, wobble, idle } => {
                let v = number(prop, stack, env, seed, wobble, idle) * scale;
                let pick = if v.is_nan() { None } else { entries.iter().rposition(|(t, _)| *t <= v) };
                let next = pick.map_or(&**fallback, |i| &entries[i].1);
                self.eval_new(next, stack, env, seed, glint, out, key);
            }
            Node::Special { base, kind, local, data } => {
                if let Some(i) = SPECIAL_KINDS.iter().position(|k| *k == kind) {
                    let tints = if data.is_empty() { stack.special_data(kind) } else { data.clone() };
                    key.push(i as u64 + 1);
                    key.push(u64::from(glint));
                    key.extend(tints.iter().map(|&t| u64::from(t)));
                    out.push(Layer { model: *base, tints: Tints::Heap(tints), local: *local, glint, special: i as u16 + 1 });
                }
            }
            Node::Missing { local } => {
                out.push(Layer { model: u32::MAX, tints: Tints::new(), local: *local, glint: 0, special: 0 })
            }
        }
    }

    /// Old path (`RUSTCRAFT_OPT_ITEMEVAL=0`): strings and `Vec` of tints allocated.
    #[allow(clippy::too_many_arguments)]
    fn eval_old(
        &self,
        node: &Node,
        stack: &dyn StackView,
        env: &dyn EvalEnv,
        seed: i32,
        glint: u8,
        out: &mut Vec<Layer>,
        key: &mut Vec<u64>,
    ) {
        key.push(node as *const Node as u64);
        match node {
            Node::Empty => {}
            Node::BundleSelected => {
                stack.bundle_selected(&mut |item, model, inner| {
                    self.layers_with(false, item, model, inner, env, seed, out, key)
                });
            }
            Node::Model { model, tints, local } => {
                let tints: Vec<u32> = tints.iter().map(|t| tint_color(t, stack, env)).collect();
                key.push(u64::from(glint));
                key.extend(tints.iter().map(|&t| u64::from(t)));
                out.push(Layer { model: *model, tints: Tints::Heap(tints), local: *local, glint, special: 0 });
            }
            Node::Composite(list) => {
                for n in list {
                    self.eval_old(n, stack, env, seed, glint, out, key);
                }
            }
            Node::Condition { prop, on_true, on_false } => {
                let v = cond(prop, stack, env);
                self.eval_old(if v { on_true } else { on_false }, stack, env, seed, glint, out, key);
            }
            Node::Select { prop, cases, fallback } => {
                let v = select(prop, stack, env);
                let next = v
                    .and_then(|v| cases.iter().find(|(w, _)| w.iter().any(|x| norm_eq(x, &v))).map(|(_, n)| n))
                    .unwrap_or(fallback);
                self.eval_old(next, stack, env, seed, glint, out, key);
            }
            Node::Range { prop, scale, entries, fallback, wobble, idle } => {
                let v = number(prop, stack, env, seed, wobble, idle) * scale;
                let pick = if v.is_nan() { None } else { entries.iter().rposition(|(t, _)| *t <= v) };
                let next = pick.map_or(&**fallback, |i| &entries[i].1);
                self.eval_old(next, stack, env, seed, glint, out, key);
            }
            // Special model: base model (display transformations) and geometry drawn
            // by the icon atlas; unsupported kinds: nothing.
            Node::Special { base, kind, local, data } => {
                if let Some(i) = SPECIAL_KINDS.iter().position(|k| *k == kind) {
                    let tints = if data.is_empty() { stack.special_data(kind) } else { data.clone() };
                    key.push(i as u64 + 1);
                    key.push(u64::from(glint));
                    key.extend(tints.iter().map(|&t| u64::from(t)));
                    out.push(Layer { model: *base, tints: Tints::Heap(tints), local: *local, glint, special: i as u16 + 1 });
                }
            }
            Node::Missing { local } => {
                out.push(Layer { model: u32::MAX, tints: Tints::Heap(Vec::new()), local: *local, glint: 0, special: 0 })
            }
        }
    }

    /// Baked model of a layer (`u32::MAX`: missing model, baked separately).
    pub fn baked(&self, model: u32) -> Option<&BakedItem> {
        self.baked.get(model as usize)
    }
}

fn norm_eq(a: &str, b: &str) -> bool {
    a == b || (a.contains(':') || b.contains(':')) && norm(a) == norm(b)
}

/// `norm_eq` without a string: `norm` reassembles the namespace (without `:`) and the path, so two
/// normalised forms are equal exactly when their splits are.
fn norm_eq_fast(a: &str, b: &str) -> bool {
    a == b || split_id(a) == split_id(b)
}

/// `f(norm(id))` without allocation: `id` as is if it has a namespace (`norm` returns it
/// identical), otherwise `minecraft:` + `id` composed on the stack (on the heap beyond 128 bytes).
fn with_norm<R>(id: &str, f: impl FnOnce(&str) -> R) -> R {
    if id.contains(':') {
        return f(id);
    }
    const NS: &[u8] = b"minecraft:";
    let mut buf = [0u8; 128];
    match buf.get_mut(..NS.len() + id.len()) {
        Some(b) => {
            b[..NS.len()].copy_from_slice(NS);
            b[NS.len()..].copy_from_slice(id.as_bytes());
            f(std::str::from_utf8(b).unwrap_or_default())
        }
        None => f(&norm(id)),
    }
}

/// `select` without copying the values that are fixed or held by the environment.
fn select_cow<'e>(p: &SelectProp, s: &dyn StackView, env: &'e dyn EvalEnv) -> Option<Cow<'e, str>> {
    match p {
        SelectProp::MainHand => {
            env.living_holder().then(|| Cow::Borrowed(if env.right_handed() { "right" } else { "left" }))
        }
        SelectProp::ChargeType => Some(Cow::Borrowed(s.charge_type())),
        SelectProp::DisplayContext => Some(Cow::Borrowed(env.display_context())),
        SelectProp::ContextEntityType => env.holder_type().map(Cow::Owned),
        SelectProp::CustomModelData(i) => s.custom_model_data_string(*i).map(Cow::Owned),
        SelectProp::TrimMaterial => s.trim_material().map(Cow::Owned),
        SelectProp::BlockState(prop) => s.block_state(prop).map(Cow::Owned),
        SelectProp::LocalTime { .. } => local_time::with_value(p, env, |v| v.map(|v| Cow::Owned(v.to_owned()))),
        SelectProp::ContextDimension => env.dimension().map(Cow::Owned),
        SelectProp::Component(_) => None,
    }
}

/// Watched environment (`RUSTCRAFT_OPT_ITEMENVFLAG`): each read raises `read`.
struct WatchEnv<'a> {
    e: &'a dyn EvalEnv,
    read: &'a Cell<bool>,
}

impl EvalEnv for WatchEnv<'_> {
    fn has_holder(&self) -> bool {
        self.read.set(true);
        self.e.has_holder()
    }
    fn living_holder(&self) -> bool {
        self.read.set(true);
        self.e.living_holder()
    }
    fn holder_type(&self) -> Option<String> {
        self.read.set(true);
        self.e.holder_type()
    }
    fn dimension(&self) -> Option<String> {
        self.read.set(true);
        self.e.dimension()
    }
    fn tick(&self) -> i64 {
        self.read.set(true);
        self.e.tick()
    }
    fn sun_angle(&self) -> f32 {
        self.read.set(true);
        self.e.sun_angle()
    }
    fn moon_phase(&self) -> f32 {
        self.read.set(true);
        self.e.moon_phase()
    }
    fn random(&self) -> f32 {
        self.read.set(true);
        self.e.random()
    }
    fn now_ms(&self) -> Option<i64> {
        self.read.set(true);
        self.e.now_ms()
    }
    fn uptime_ms(&self) -> i64 {
        self.read.set(true);
        self.e.uptime_ms()
    }
    fn utc_offset_secs(&self, ms: i64) -> i64 {
        self.read.set(true);
        self.e.utc_offset_secs(ms)
    }
    fn last_death(&self) -> Option<(String, [i32; 3])> {
        self.read.set(true);
        self.e.last_death()
    }
    fn key_down(&self, keybind: &str) -> bool {
        self.read.set(true);
        self.e.key_down(keybind)
    }
    fn view_entity(&self) -> bool {
        self.read.set(true);
        self.e.view_entity()
    }
    fn shift_down(&self) -> bool {
        self.read.set(true);
        self.e.shift_down()
    }
    fn right_handed(&self) -> bool {
        self.read.set(true);
        self.e.right_handed()
    }
    fn display_context(&self) -> &str {
        self.read.set(true);
        self.e.display_context()
    }
    fn holder_pos(&self) -> [f64; 3] {
        self.read.set(true);
        self.e.holder_pos()
    }
    fn body_yaw(&self) -> f32 {
        self.read.set(true);
        self.e.body_yaw()
    }
    fn spawn(&self) -> Option<(String, [i32; 3])> {
        self.read.set(true);
        self.e.spawn()
    }
    fn local_ticking(&self) -> bool {
        self.read.set(true);
        self.e.local_ticking()
    }
    fn team_color(&self) -> Option<u32> {
        self.read.set(true);
        self.e.team_color()
    }
}

/// Watched stack: the holder's state read through the stack (use in progress, selected slot) raises
/// `read`; the rest depends only on the stack's content.
struct WatchStack<'a> {
    s: &'a dyn StackView,
    read: &'a Cell<bool>,
}

impl StackView for WatchStack<'_> {
    fn has(&self, component: &str) -> bool {
        self.s.has(component)
    }
    fn patched(&self, component: &str) -> bool {
        self.s.patched(component)
    }
    fn damage(&self) -> i32 {
        self.s.damage()
    }
    fn max_damage(&self) -> i32 {
        self.s.max_damage()
    }
    fn count(&self) -> i32 {
        self.s.count()
    }
    fn max_stack_size(&self) -> i32 {
        self.s.max_stack_size()
    }
    fn dyed_color(&self) -> Option<u32> {
        self.s.dyed_color()
    }
    fn potion_color(&self) -> Option<u32> {
        self.s.potion_color()
    }
    fn firework_colors(&self, f: &mut dyn FnMut(u32)) {
        self.s.firework_colors(f)
    }
    fn custom_model_data_flag(&self, i: usize) -> Option<bool> {
        self.s.custom_model_data_flag(i)
    }
    fn custom_model_data_float(&self, i: usize) -> Option<f32> {
        self.s.custom_model_data_float(i)
    }
    fn custom_model_data_string(&self, i: usize) -> Option<String> {
        self.s.custom_model_data_string(i)
    }
    fn custom_model_data_color(&self, i: usize) -> Option<u32> {
        self.s.custom_model_data_color(i)
    }
    fn special_data(&self, kind: &str) -> Vec<u32> {
        self.s.special_data(kind)
    }
    fn block_state(&self, property: &str) -> Option<String> {
        self.s.block_state(property)
    }
    fn charge_type(&self) -> &'static str {
        self.s.charge_type()
    }
    fn crossbow_charge(&self) -> i32 {
        self.s.crossbow_charge()
    }
    fn trim_material(&self) -> Option<String> {
        self.s.trim_material()
    }
    fn lodestone(&self) -> Option<(String, [i32; 3])> {
        self.s.lodestone()
    }
    fn glint(&self) -> bool {
        self.s.glint()
    }
    fn selected(&self) -> bool {
        self.read.set(true);
        self.s.selected()
    }
    fn using(&self) -> Option<(i32, i32)> {
        self.read.set(true);
        self.s.using()
    }
    fn fishing_cast(&self) -> bool {
        self.read.set(true);
        self.s.fishing_cast()
    }
    fn carried(&self) -> bool {
        self.read.set(true);
        self.s.carried()
    }
    // The selected item is client state (scroll wheel): it raises the flag, and the selected stack is
    // watched in turn.
    fn bundle_selected(&self, f: &mut dyn FnMut(&str, &str, &dyn StackView)) -> bool {
        self.read.set(true);
        self.s.bundle_selected(&mut |item, model, s| f(item, model, &WatchStack { s, read: self.read }))
    }
}

fn tint_color(t: &Tint, stack: &dyn StackView, env: &dyn EvalEnv) -> u32 {
    match *t {
        Tint::Constant(c) | Tint::Grass(c) => c,
        // Holder's team (read only for this tint: the environment is not marked as read
        // for the other items).
        Tint::Team(d) => env.team_color().map_or(d, |c| c | 0xff00_0000),
        Tint::Dye(d) => stack.dyed_color().map_or(d, |c| c | 0xff00_0000),
        // Default rendered as is; one colour: opaque; several: average truncated per channel.
        Tint::Firework(d) => {
            let (mut n, mut first, mut sum) = (0u64, 0u32, [0u64; 3]);
            stack.firework_colors(&mut |c| {
                if n == 0 {
                    first = c;
                }
                n += 1;
                for (k, s) in sum.iter_mut().enumerate() {
                    *s += u64::from((c >> (16 - 8 * k)) & 0xff);
                }
            });
            match n {
                0 => d,
                1 => first | 0xff00_0000,
                n => sum.iter().fold(0xffu32, |a, s| a << 8 | (s / n) as u32),
            }
        }
        Tint::Potion(d) => stack.potion_color().map_or(d, |c| c | 0xff00_0000),
        Tint::CustomModelData { index, default } => {
            stack.custom_model_data_color(index).map_or(default, |c| c | 0xff00_0000)
        }
    }
}

/// Usable stack: maximum damage and damage present, and not unbreakable.
fn damageable(s: &dyn StackView) -> bool {
    s.has("minecraft:max_damage") && s.has("minecraft:damage") && !s.has("minecraft:unbreakable")
}

/// Damage bounded to [0, max].
fn damage_value(s: &dyn StackView) -> i32 {
    s.damage().clamp(0, s.max_damage().max(0))
}

fn in_bounds(v: i32, [lo, hi]: [Option<i32>; 2]) -> bool {
    lo.is_none_or(|l| v >= l) && hi.is_none_or(|h| v <= h)
}

fn component_matches(p: &ComponentPred, s: &dyn StackView) -> bool {
    match p {
        // Damage present (default included); durability = max − raw damage.
        ComponentPred::Damage { durability, damage } => {
            s.has("minecraft:damage") && {
                let d = s.damage();
                in_bounds(s.max_damage() - d, *durability) && in_bounds(d, *damage)
            }
        }
        ComponentPred::Present(c) => s.has(c),
        ComponentPred::Unsupported => false,
    }
}

fn cond(p: &CondProp, s: &dyn StackView, env: &dyn EvalEnv) -> bool {
    match p {
        CondProp::Broken => damageable(s) && damage_value(s) >= s.max_damage() - 1,
        CondProp::Damaged => damageable(s) && damage_value(s) > 0,
        CondProp::HasComponent { component, ignore_default } => {
            if *ignore_default {
                s.patched(component)
            } else {
                s.has(component)
            }
        }
        CondProp::Selected => env.living_holder() && s.selected(),
        CondProp::ExtendedView => env.shift_down(),
        CondProp::ViewEntity => env.living_holder() && env.view_entity(),
        CondProp::CustomModelData(i) => s.custom_model_data_flag(*i).unwrap_or(false),
        CondProp::UsingItem => s.using().is_some(),
        CondProp::FishingRodCast => env.living_holder() && s.fishing_cast(),
        CondProp::BundleHasSelected => s.bundle_selected(&mut |_, _, _| {}),
        CondProp::Carried => env.living_holder() && s.carried(),
        CondProp::KeybindDown(k) => env.key_down(k),
        CondProp::Component(c) => component_matches(c, s),
    }
}

fn select(p: &SelectProp, s: &dyn StackView, env: &dyn EvalEnv) -> Option<String> {
    match p {
        SelectProp::CustomModelData(i) => s.custom_model_data_string(*i),
        SelectProp::MainHand => env.living_holder().then(|| if env.right_handed() { "right" } else { "left" }.to_owned()),
        SelectProp::ChargeType => Some(s.charge_type().to_owned()),
        SelectProp::TrimMaterial => s.trim_material(),
        SelectProp::BlockState(prop) => s.block_state(prop),
        SelectProp::DisplayContext => Some(env.display_context().to_owned()),
        SelectProp::LocalTime { .. } => local_time::with_value(p, env, |v| v.map(str::to_owned)),
        SelectProp::ContextEntityType => env.holder_type(),
        SelectProp::ContextDimension => env.dimension(),
        SelectProp::Component(_) => None,
    }
}

fn number(p: &NumProp, s: &dyn StackView, env: &dyn EvalEnv, seed: i32, wobble: &Wobbler, idle: &Wobbler) -> f32 {
    match *p {
        NumProp::CustomModelData(i) => s.custom_model_data_float(i).unwrap_or(0.0),
        NumProp::Damage { normalize } => {
            let m = s.max_damage().max(0) as f32;
            let d = (s.damage() as f32).clamp(0.0, m);
            if normalize { (d / m).clamp(0.0, 1.0) } else { d.clamp(0.0, m) }
        }
        NumProp::Count { normalize } => {
            let (n, m) = (s.count() as f32, s.max_stack_size().max(1) as f32);
            if normalize { (n / m).clamp(0.0, 1.0) } else { n.clamp(0.0, m) }
        }
        NumProp::Time { wobble: w, source } => {
            if !env.has_holder() {
                return 0.0;
            }
            let target = match source {
                TimeSource::Daytime => env.sun_angle() / 360.0,
                TimeSource::MoonPhase => env.moon_phase() / 8.0,
                TimeSource::Random => env.random(),
            };
            if w { wobble.update(env.tick(), target, 0.9) } else { target }
        }
        NumProp::Compass { wobble: w, target } => {
            if !env.has_holder() {
                return 0.0;
            }
            let goal = match target {
                CompassTarget::Lodestone => s.lodestone(),
                CompassTarget::Spawn => env.spawn(),
                CompassTarget::Recovery => env.last_death(),
                CompassTarget::None => None,
            };
            let p = env.holder_pos();
            let valid = goal.filter(|(dim, b)| {
                let (dx, dz) = (f64::from(b[0]) + 0.5 - p[0], f64::from(b[2]) + 0.5 - p[2]);
                let dy = f64::from(b[1]) + 0.5 - p[1];
                env.dimension().is_some_and(|d| d == *dim) && dx * dx + dy * dy + dz * dz >= 1.0e-5
            });
            match valid {
                // Invalid target: random rotation, offset by the slot's seed.
                None => {
                    let rot = if w { idle.update(env.tick(), env.random(), 0.8) } else { env.random() };
                    let h = seed.wrapping_mul(1_327_217_883) as f32;
                    positive_mod(rot + h / 2.147_483_6e9, 1.0)
                }
                Some((_, b)) => {
                    let a = ((f64::from(b[2]) + 0.5 - p[2]).atan2(f64::from(b[0]) + 0.5 - p[0]) / std::f64::consts::TAU)
                        as f32;
                    let r = positive_mod(env.body_yaw() / 360.0, 1.0);
                    if env.local_ticking() {
                        let t = 0.5 - (r - 0.25);
                        let rot = if w { wobble.update(env.tick(), t, 0.8) } else { t };
                        positive_mod(a + rot, 1.0)
                    } else {
                        positive_mod(0.5 - (r - 0.25 - a), 1.0)
                    }
                }
            }
        }
        NumProp::UseDuration { remaining } => {
            s.using().map_or(0.0, |(used, left)| if remaining { left as f32 } else { used as f32 })
        }
        NumProp::UseCycle(period) => s.using().map_or(0.0, |(_, left)| (left as f32) % period),
        // Crossbow charging: ticks of use over the charge time (Quick Charge included), 0
        // if loaded.
        NumProp::CrossbowPull => {
            if s.charge_type() != "none" {
                return 0.0;
            }
            s.using().map_or(0.0, |(used, _)| used as f32 / s.crossbow_charge() as f32)
        }
        NumProp::BundleFullness | NumProp::Cooldown => 0.0,
    }
}

mod local_time;

// Resource cache (`cache.rs`): baked definitions and models.
mod codec {
    use super::*;
    use crate::cache::{Codec, R, W};

    fn tag(w: &mut W, t: u8) {
        t.enc(w);
    }

    impl Codec for Tint {
        fn enc(&self, w: &mut W) {
            match self {
                Tint::Constant(c) => (tag(w, 0), c.enc(w)).1,
                Tint::Dye(c) => (tag(w, 1), c.enc(w)).1,
                Tint::Grass(c) => (tag(w, 2), c.enc(w)).1,
                Tint::Firework(c) => (tag(w, 3), c.enc(w)).1,
                Tint::Potion(c) => (tag(w, 4), c.enc(w)).1,
                Tint::Team(c) => (tag(w, 5), c.enc(w)).1,
                Tint::CustomModelData { index, default } => {
                    tag(w, 6);
                    index.enc(w);
                    default.enc(w);
                }
            }
        }
        fn dec(r: &mut R<'_>) -> Option<Self> {
            Some(match u8::dec(r)? {
                0 => Tint::Constant(Codec::dec(r)?),
                1 => Tint::Dye(Codec::dec(r)?),
                2 => Tint::Grass(Codec::dec(r)?),
                3 => Tint::Firework(Codec::dec(r)?),
                4 => Tint::Potion(Codec::dec(r)?),
                5 => Tint::Team(Codec::dec(r)?),
                6 => Tint::CustomModelData { index: Codec::dec(r)?, default: Codec::dec(r)? },
                _ => return None,
            })
        }
    }

    impl Codec for CondProp {
        fn enc(&self, w: &mut W) {
            match self {
                CondProp::UsingItem => tag(w, 0),
                CondProp::Broken => tag(w, 1),
                CondProp::Damaged => tag(w, 2),
                CondProp::HasComponent { component, ignore_default } => {
                    tag(w, 3);
                    component.enc(w);
                    ignore_default.enc(w);
                }
                CondProp::FishingRodCast => tag(w, 4),
                CondProp::BundleHasSelected => tag(w, 5),
                CondProp::Selected => tag(w, 6),
                CondProp::Carried => tag(w, 7),
                CondProp::ExtendedView => tag(w, 8),
                CondProp::KeybindDown(k) => (tag(w, 9), k.enc(w)).1,
                CondProp::ViewEntity => tag(w, 10),
                CondProp::CustomModelData(i) => (tag(w, 11), i.enc(w)).1,
                CondProp::Component(p) => (tag(w, 12), p.enc(w)).1,
            }
        }
        fn dec(r: &mut R<'_>) -> Option<Self> {
            Some(match u8::dec(r)? {
                0 => CondProp::UsingItem,
                1 => CondProp::Broken,
                2 => CondProp::Damaged,
                3 => CondProp::HasComponent { component: Codec::dec(r)?, ignore_default: Codec::dec(r)? },
                4 => CondProp::FishingRodCast,
                5 => CondProp::BundleHasSelected,
                6 => CondProp::Selected,
                7 => CondProp::Carried,
                8 => CondProp::ExtendedView,
                9 => CondProp::KeybindDown(Codec::dec(r)?),
                10 => CondProp::ViewEntity,
                11 => CondProp::CustomModelData(Codec::dec(r)?),
                12 => CondProp::Component(Codec::dec(r)?),
                _ => return None,
            })
        }
    }

    impl Codec for ComponentPred {
        fn enc(&self, w: &mut W) {
            match self {
                ComponentPred::Damage { durability, damage } => {
                    tag(w, 0);
                    for b in durability.iter().chain(damage) {
                        b.is_some().enc(w);
                        b.unwrap_or(0).enc(w);
                    }
                }
                ComponentPred::Present(c) => (tag(w, 1), c.enc(w)).1,
                ComponentPred::Unsupported => tag(w, 2),
            }
        }
        fn dec(r: &mut R<'_>) -> Option<Self> {
            Some(match u8::dec(r)? {
                0 => {
                    let mut b = [None; 4];
                    for v in &mut b {
                        let some = bool::dec(r)?;
                        let n = i32::dec(r)?;
                        *v = some.then_some(n);
                    }
                    ComponentPred::Damage { durability: [b[0], b[1]], damage: [b[2], b[3]] }
                }
                1 => ComponentPred::Present(Codec::dec(r)?),
                2 => ComponentPred::Unsupported,
                _ => return None,
            })
        }
    }

    impl Codec for TimeZone {
        fn enc(&self, w: &mut W) {
            match self {
                TimeZone::System => tag(w, 0),
                TimeZone::Fixed(s) => (tag(w, 1), s.enc(w)).1,
            }
        }
        fn dec(r: &mut R<'_>) -> Option<Self> {
            Some(match u8::dec(r)? {
                0 => TimeZone::System,
                1 => TimeZone::Fixed(Codec::dec(r)?),
                _ => return None,
            })
        }
    }

    impl Codec for SelectProp {
        fn enc(&self, w: &mut W) {
            match self {
                SelectProp::CustomModelData(i) => (tag(w, 0), i.enc(w)).1,
                SelectProp::MainHand => tag(w, 1),
                SelectProp::ChargeType => tag(w, 2),
                SelectProp::TrimMaterial => tag(w, 3),
                SelectProp::BlockState(s) => (tag(w, 4), s.enc(w)).1,
                SelectProp::DisplayContext => tag(w, 5),
                SelectProp::LocalTime { pattern, locale, zone, .. } => {
                    tag(w, 6);
                    pattern.enc(w);
                    locale.enc(w);
                    zone.enc(w);
                }
                SelectProp::ContextEntityType => tag(w, 7),
                SelectProp::ContextDimension => tag(w, 8),
                SelectProp::Component(s) => (tag(w, 9), s.enc(w)).1,
            }
        }
        fn dec(r: &mut R<'_>) -> Option<Self> {
            Some(match u8::dec(r)? {
                0 => SelectProp::CustomModelData(Codec::dec(r)?),
                1 => SelectProp::MainHand,
                2 => SelectProp::ChargeType,
                3 => SelectProp::TrimMaterial,
                4 => SelectProp::BlockState(Codec::dec(r)?),
                5 => SelectProp::DisplayContext,
                6 => SelectProp::LocalTime {
                    pattern: Codec::dec(r)?,
                    locale: Codec::dec(r)?,
                    zone: Codec::dec(r)?,
                    cache: TimeCache::default(),
                },
                7 => SelectProp::ContextEntityType,
                8 => SelectProp::ContextDimension,
                9 => SelectProp::Component(Codec::dec(r)?),
                _ => return None,
            })
        }
    }

    crate::codec_enum!(TimeSource { Daytime, MoonPhase, Random });
    crate::codec_enum!(CompassTarget { None, Lodestone, Spawn, Recovery });

    impl Codec for NumProp {
        fn enc(&self, w: &mut W) {
            match self {
                NumProp::CustomModelData(i) => (tag(w, 0), i.enc(w)).1,
                NumProp::BundleFullness => tag(w, 1),
                NumProp::Damage { normalize } => (tag(w, 2), normalize.enc(w)).1,
                NumProp::Cooldown => tag(w, 3),
                NumProp::Time { wobble, source } => {
                    tag(w, 4);
                    wobble.enc(w);
                    source.enc(w);
                }
                NumProp::Compass { wobble, target } => {
                    tag(w, 5);
                    wobble.enc(w);
                    target.enc(w);
                }
                NumProp::CrossbowPull => tag(w, 6),
                NumProp::UseCycle(f) => (tag(w, 7), f.enc(w)).1,
                NumProp::UseDuration { remaining } => (tag(w, 8), remaining.enc(w)).1,
                NumProp::Count { normalize } => (tag(w, 9), normalize.enc(w)).1,
            }
        }
        fn dec(r: &mut R<'_>) -> Option<Self> {
            Some(match u8::dec(r)? {
                0 => NumProp::CustomModelData(Codec::dec(r)?),
                1 => NumProp::BundleFullness,
                2 => NumProp::Damage { normalize: Codec::dec(r)? },
                3 => NumProp::Cooldown,
                4 => NumProp::Time { wobble: Codec::dec(r)?, source: Codec::dec(r)? },
                5 => NumProp::Compass { wobble: Codec::dec(r)?, target: Codec::dec(r)? },
                6 => NumProp::CrossbowPull,
                7 => NumProp::UseCycle(Codec::dec(r)?),
                8 => NumProp::UseDuration { remaining: Codec::dec(r)? },
                9 => NumProp::Count { normalize: Codec::dec(r)? },
                _ => return None,
            })
        }
    }

    impl Codec for Node {
        fn enc(&self, w: &mut W) {
            match self {
                Node::Empty => tag(w, 0),
                Node::Model { model, tints, local } => {
                    tag(w, 1);
                    model.enc(w);
                    tints.enc(w);
                    local.enc(w);
                }
                Node::Composite(v) => (tag(w, 2), v.enc(w)).1,
                Node::Condition { prop, on_true, on_false } => {
                    tag(w, 3);
                    prop.enc(w);
                    on_true.enc(w);
                    on_false.enc(w);
                }
                Node::Select { prop, cases, fallback } => {
                    tag(w, 4);
                    prop.enc(w);
                    cases.enc(w);
                    fallback.enc(w);
                }
                // Oscillators: run-time state, restart from zero.
                Node::Range { prop, scale, entries, fallback, .. } => {
                    tag(w, 5);
                    prop.enc(w);
                    scale.enc(w);
                    entries.enc(w);
                    fallback.enc(w);
                }
                Node::Special { base, kind, local, data } => {
                    tag(w, 6);
                    base.enc(w);
                    kind.enc(w);
                    local.enc(w);
                    data.enc(w);
                }
                Node::BundleSelected => tag(w, 7),
                Node::Missing { local } => (tag(w, 8), local.enc(w)).1,
            }
        }
        fn dec(r: &mut R<'_>) -> Option<Self> {
            Some(match u8::dec(r)? {
                0 => Node::Empty,
                1 => Node::Model { model: Codec::dec(r)?, tints: Codec::dec(r)?, local: Codec::dec(r)? },
                2 => Node::Composite(Codec::dec(r)?),
                3 => Node::Condition { prop: Codec::dec(r)?, on_true: Codec::dec(r)?, on_false: Codec::dec(r)? },
                4 => Node::Select { prop: Codec::dec(r)?, cases: Codec::dec(r)?, fallback: Codec::dec(r)? },
                5 => Node::Range {
                    prop: Codec::dec(r)?,
                    scale: Codec::dec(r)?,
                    entries: Codec::dec(r)?,
                    fallback: Codec::dec(r)?,
                    wobble: Wobbler::default(),
                    idle: Wobbler::default(),
                },
                6 => Node::Special {
                    base: Codec::dec(r)?,
                    kind: Codec::dec(r)?,
                    local: Codec::dec(r)?,
                    data: Codec::dec(r)?,
                },
                7 => Node::BundleSelected,
                8 => Node::Missing { local: Codec::dec(r)? },
                _ => return None,
            })
        }
    }

    crate::codec_struct!(ItemQuad { pos, uv, sprite, tint, dir, translucent });
    crate::codec_struct!(PackedLayer { sprite, layer, translucent, w, h, edges });

    /// Quads in columns: positions, UVs (numbers in bulk), sprites, tints, directions, transparency.
    pub(super) fn enc_cols(q: &[ItemQuad], w: &mut W) {
        q.len().enc(w);
        for q in q {
            q.pos.enc(w);
        }
        for q in q {
            q.uv.enc(w);
        }
        for q in q {
            q.sprite.enc(w);
        }
        for q in q {
            q.tint.enc(w);
        }
        for q in q {
            q.dir.enc(w);
        }
        for q in q {
            q.translucent.enc(w);
        }
    }

    fn f32s<const N: usize>(c: &[u8]) -> [f32; N] {
        let mut it = c.as_chunks::<4>().0.iter();
        std::array::from_fn(|_| it.next().map_or(0.0, |b| f32::from_le_bytes(*b)))
    }

    pub(super) fn dec_cols(r: &mut R<'_>) -> Option<Vec<ItemQuad>> {
        let n = r.len()?;
        let pos = r.take(n.checked_mul(48)?)?;
        let uv = r.take(n.checked_mul(32)?)?;
        let sprite = r.take(n.checked_mul(4)?)?;
        let tint = r.take(n.checked_mul(4)?)?;
        let dir = r.take(n)?;
        let translucent = r.take(n)?;
        let mut out = Vec::with_capacity(n);
        let cols = pos.as_chunks::<48>().0.iter().zip(uv.as_chunks::<32>().0);
        let ids = sprite.as_chunks::<4>().0.iter().zip(tint.as_chunks::<4>().0);
        for (((p, u), (s, t)), (&d, &tr)) in cols.zip(ids).zip(dir.iter().zip(translucent)) {
            let p: [f32; 12] = f32s(p);
            let u: [f32; 8] = f32s(u);
            out.push(ItemQuad {
                pos: std::array::from_fn(|i| [p[i * 3], p[i * 3 + 1], p[i * 3 + 2]]),
                uv: std::array::from_fn(|i| [u[i * 2], u[i * 2 + 1]]),
                sprite: u32::from_le_bytes(*s),
                tint: i32::from_le_bytes(*t),
                dir: *Dir::ALL.get(usize::from(d))?,
                translucent: match tr {
                    0 => false,
                    1 => true,
                    _ => return None,
                },
            });
        }
        Some(out)
    }

    /// Leading shape: 0 quads row by row, 1 quads in columns, 2 pixel layers. Each
    /// shape is read back whatever the switch; a re-read is rewritten identically.
    impl Codec for BakedItem {
        fn enc(&self, w: &mut W) {
            if !self.packed.is_empty() {
                tag(w, 2);
                self.packed.enc(w);
            } else if self.form == 1 {
                tag(w, 1);
                enc_cols(self.quads(), w);
            } else {
                tag(w, 0);
                let q = self.quads();
                q.len().enc(w);
                for q in q {
                    q.enc(w);
                }
            }
            self.gui_front.enc(w);
            self.display.enc(w);
            self.displays.enc(w);
            self.animated.enc(w);
            self.particle.enc(w);
        }
        fn dec(r: &mut R<'_>) -> Option<Self> {
            let form = u8::dec(r)?;
            let (quads, packed) = match form {
                0 => (OnceLock::from(Vec::<ItemQuad>::dec(r)?), Vec::new()),
                1 => (OnceLock::from(dec_cols(r)?), Vec::new()),
                2 => (OnceLock::new(), Vec::<PackedLayer>::dec(r)?),
                _ => return None,
            };
            let b = BakedItem {
                quads,
                packed,
                form,
                gui_front: Codec::dec(r)?,
                display: Codec::dec(r)?,
                displays: Codec::dec(r)?,
                animated: Codec::dec(r)?,
                particle: Codec::dec(r)?,
            };
            // Old path (`RUSTCRAFT_OPT_ITEMQUADPACK=0`): quads unfolded as soon as re-read.
            if !Opt::get().quad_pack {
                b.quads();
            }
            Some(b)
        }
    }

    impl Codec for ItemModels {
        fn enc(&self, w: &mut W) {
            self.defs.enc(w);
            self.swap.enc(w);
            self.baked.enc(w);
            self.sprites.enc(w);
            self.sprite_names.enc(w);
            self.animations.enc(w);
            self.frames.enc(w);
            self.compasses.enc(w);
            self.missing.enc(w);
            self.glint.enc(w);
        }
        fn dec(r: &mut R<'_>) -> Option<Self> {
            Some(ItemModels {
                defs: Codec::dec(r)?,
                swap: Codec::dec(r)?,
                baked: Codec::dec(r)?,
                sprites: Codec::dec(r)?,
                sprite_names: Codec::dec(r)?,
                animations: Codec::dec(r)?,
                frames: Codec::dec(r)?,
                compasses: Codec::dec(r)?,
                missing: Codec::dec(r)?,
                glint: Codec::dec(r)?,
                generation: next_generation(),
            })
        }
    }
}

#[cfg(test)]
mod tests_opt;

#[cfg(test)]
mod tests_g2f04;

#[cfg(test)]
mod tests_g1f11;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_of_block_item() {
        let d = crate::model::Display { rotation: [30.0, 225.0, 0.0], translation: [0.0; 3], scale: [0.625; 3] };
        let m = display_matrix(Some(&d));
        // The centre of the block stays at the origin.
        let c = apply(&m, [0.5, 0.5, 0.5]);
        assert!(c.iter().all(|v| v.abs() < 1e-6));
        assert_eq!(display_matrix(None), translate([-0.5; 3]));
    }

    /// "particle" sprite of the break particles (installed jar 26.3): `particle` texture of the model
    /// (shield, chest, stairs), `layer0` of a flat model (enchanted golden apple, trident
    /// placed on the ground).
    #[test]
    fn particle_sprite_of_models() {
        struct Ground;
        impl StackView for Ground {
            fn has(&self, _: &str) -> bool {
                false
            }
        }
        impl EvalEnv for Ground {
            fn has_holder(&self) -> bool {
                false
            }
            fn display_context(&self) -> &str {
                "ground"
            }
        }
        #[allow(clippy::disallowed_methods)]
        let jar = std::path::PathBuf::from(std::env::var_os("APPDATA").expect("APPDATA"))
            .join(".minecraft/versions/26.3/26.3.jar");
        let assets = crate::Assets::for_install(&jar, &[], None).expect("installed game 26.3 jar");
        let want = [
            ("minecraft:enchanted_golden_apple", "minecraft:item/golden_apple"),
            ("minecraft:shield", "minecraft:block/dark_oak_planks"),
            ("minecraft:oak_stairs", "minecraft:block/oak_planks"),
            ("minecraft:trident", "minecraft:item/trident"),
            ("minecraft:chest", "minecraft:block/oak_planks"),
        ];
        let m = ItemModels::load(&assets, want.iter().map(|w| w.0));
        for (id, sprite) in want {
            let (mut out, mut key) = (Vec::new(), Vec::new());
            m.layers(id, id, &Ground, &Ground, 0, &mut out, &mut key);
            let b = m.baked(out[0].model).expect(id);
            assert_eq!(m.sprite_names[b.particle as usize], sprite, "{id}");
        }
    }

    #[test]
    fn extrusion_of_single_pixel() {
        let mut rgba = vec![0u8; 2 * 2 * 4];
        rgba[3] = 255;
        let img = Image { width: 2, height: 2, rgba };
        let mut out = Vec::new();
        extrude(&img, &[], 0, 1, false, &mut out);
        // Front and back faces, then 4 edges around the single opaque pixel.
        assert_eq!(out.len(), 6);
        assert!(out.iter().all(|q| q.tint == 1));
        let top = out.iter().find(|q| q.dir == Dir::Up).unwrap();
        assert!(top.pos.iter().all(|p| p[1] == 1.0));
    }

    #[test]
    fn quaternion_and_matrix_forms() {
        let v = serde_json::json!({"translation":[0.5,0,0.5],"left_rotation":[1,0,0,0],"scale":[1,1,1],"right_rotation":[0,0,0,1]});
        let m = transformation(&v).unwrap();
        let p = apply(&m, [0.0, 1.0, 0.0]);
        assert!((p[0] - 0.5).abs() < 1e-6 && (p[1] + 1.0).abs() < 1e-6 && (p[2] - 0.5).abs() < 1e-6);
        let flat: Vec<f32> = (0..16).map(|i| if i % 5 == 0 { 1.0 } else { 0.0 }).collect();
        assert_eq!(transformation(&serde_json::json!(flat)).unwrap(), IDENTITY);
    }

    #[test]
    fn axis_angle_keeps_axis_length_and_zero_quaternion_collapses() {
        // Axis (0, 2, 0), angle 0: quaternion (0, 0, 0, 1) → identity despite the long axis.
        let id = rotation(&serde_json::json!({"angle": 0.0, "axis": [0, 2, 0]})).unwrap();
        assert_eq!(id, IDENTITY);
        // Axis (0, 2, 0), angle π: quaternion (0, 2, 0, ~0) of norm 2 → rotation × 4.
        let m = rotation(&serde_json::json!({"angle": std::f32::consts::PI, "axis": [0, 2, 0]})).unwrap();
        let p = apply(&m, [1.0, 0.0, 0.0]);
        assert!((p[0] + 4.0).abs() < 1e-5 && p[1].abs() < 1e-5 && p[2].abs() < 1e-5, "{p:?}");
        // Unit axis: pure rotation.
        let m = rotation(&serde_json::json!({"angle": std::f32::consts::FRAC_PI_2, "axis": [0, 1, 0]})).unwrap();
        let p = apply(&m, [1.0, 0.0, 0.0]);
        assert!(p[0].abs() < 1e-6 && (p[2] + 1.0).abs() < 1e-6, "{p:?}");
        // Zero quaternion in components: squashed geometry (invisible), not the identity.
        let z = rotation(&serde_json::json!([0, 0, 0, 0])).unwrap();
        assert_eq!(apply(&z, [1.0, 2.0, 3.0]), [0.0; 3]);
    }
}
