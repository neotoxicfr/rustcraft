//! Client-side entities (`docs/specs/entities.md` part 1 § 4, part 3 § 1): received state,
//! position interpolation, body rotation, limb swing, hits and death.

use hashbrown::HashMap;

use rc_client::EntityEvent;
use rc_proto_v777::entity::{Meta, PositionPath};
use rc_proto_v777::item::ItemStack;

/// Interpolation method of an entity type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interp {
    /// Living: queue of steps, default duration in ticks.
    Stepped(u32),
    /// Boats, minecarts, orbs: N linear steps.
    Linear(u32),
    /// Direct placement.
    None,
}

/// Kind of body rotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Mob: body rotation controller (75°).
    Mob,
    /// Living non-mob (player, armor stand…): 50° rule.
    Living,
    /// Non-living.
    Other,
}

const NON_LIVING: &[&str] = &[
    "item",
    "experience_orb",
    "arrow",
    "spectral_arrow",
    "trident",
    "falling_block",
    "tnt",
    "painting",
    "item_frame",
    "glow_item_frame",
    "leash_knot",
    "area_effect_cloud",
    "end_crystal",
    "fishing_bobber",
    "lightning_bolt",
    "eye_of_ender",
    "firework_rocket",
    "snowball",
    "egg",
    "ender_pearl",
    "splash_potion",
    "lingering_potion",
    "experience_bottle",
    "fireball",
    "small_fireball",
    "dragon_fireball",
    "wither_skull",
    "shulker_bullet",
    "llama_spit",
    "evoker_fangs",
    "marker",
    "interaction",
    "wind_charge",
    "breeze_wind_charge",
    "ominous_item_spawner",
    "block_display",
    "item_display",
    "text_display",
    "minecart",
    "chest_minecart",
    "furnace_minecart",
    "hopper_minecart",
    "spawner_minecart",
    "tnt_minecart",
    "command_block_minecart",
];

/// Interpolation method and kind of a type (name without namespace).
pub fn classify(name: &str) -> (Interp, Kind) {
    let boat = name.ends_with("boat") || name.ends_with("raft");
    if boat || name.ends_with("minecart") || name == "experience_orb" || name == "fishing_bobber" {
        return (Interp::Linear(3), Kind::Other);
    }
    // Displays: linear over the duration received (data 10), direct placement at 0.
    if matches!(name, "block_display" | "item_display" | "text_display") {
        return (Interp::Linear(0), Kind::Other);
    }
    // Shulker: never interpolated (it moves from one block to another).
    if name == "shulker" {
        return (Interp::None, Kind::Mob);
    }
    if NON_LIVING.contains(&name) {
        return (Interp::None, Kind::Other);
    }
    match name {
        "player" | "mannequin" => (Interp::Stepped(2), Kind::Living),
        "armor_stand" => (Interp::Stepped(3), Kind::Living),
        "allay" => (Interp::Stepped(2), Kind::Mob),
        _ => (Interp::Stepped(3), Kind::Mob),
    }
}

/// Dimensions of an entity: width, height, eye height, name attachment; `fixed`:
/// insensitive to scaling (sleeping, dying, marker).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub w: f32,
    pub h: f32,
    pub eye: f32,
    pub tag: f32,
    pub fixed: bool,
}

impl Size {
    /// Eyes at 0.85 × height, name at the top.
    pub fn scalable(w: f32, h: f32) -> Size {
        Size { w, h, eye: h * 0.85, tag: h, fixed: false }
    }

    pub fn fixed(w: f32, h: f32) -> Size {
        Size { fixed: true, ..Size::scalable(w, h) }
    }

    pub fn eye(self, eye: f32) -> Size {
        Size { eye, ..self }
    }

    pub fn scale(self, k: f32) -> Size {
        self.scale2(k, k)
    }

    /// Width × `kw`, heights × `kh` (no effect on fixed dimensions).
    pub fn scale2(self, kw: f32, kh: f32) -> Size {
        if self.fixed || (kw == 1.0 && kh == 1.0) {
            return self;
        }
        Size { w: self.w * kw, h: self.h * kh, eye: self.eye * kh, tag: self.tag * kh, fixed: false }
    }
}

/// Dimensions of a baby: specific to the type (fixed size, or type scaled with given
/// eyes), otherwise type × 1/2. `ty`: adult dimensions; `pose`: current pose.
fn baby_size(kind: &str, ty: Size, pose: i32) -> Size {
    let own = |w: f32, h: f32, eye: f32| Size::scalable(w, h).eye(eye);
    // Scaled type whose attachments are rebuilt: name at the top.
    let rebuilt = |k: f32| {
        let s = ty.scale(k);
        Size { tag: s.h, ..s }
    };
    match kind {
        "armadillo" => ty.scale(0.6).eye(0.218_75),
        "axolotl" => own(0.375, 0.21, 0.093_75),
        "camel" | "camel_husk" if pose != 10 => own(0.95, 1.4, 1.38),
        "chicken" => own(0.3, 0.4, 0.281_25),
        "cow" | "mooshroom" => own(0.45, 0.7, 0.69),
        "dolphin" => {
            let s = ty.scale(0.65).eye(0.093_75);
            Size { tag: s.h, ..s }
        }
        "donkey" | "mule" | "llama" | "trader_llama" => rebuilt(0.5),
        "horse" | "skeleton_horse" | "zombie_horse" => rebuilt(0.7),
        "cat" | "ocelot" => own(0.3, 0.35, 0.343_75),
        "fox" => {
            let s = ty.scale(0.6).eye(0.343_75);
            Size { tag: s.h, ..s }
        }
        "goat" => own(0.45, 0.65, 0.593_75),
        "happy_ghast" => ty.scale(0.2375).eye(0.468_75),
        "nautilus" => rebuilt(0.5),
        "panda" => {
            let s = ty.scale(0.5).eye(0.281_25);
            Size { tag: s.h, ..s }
        }
        "pig" => own(0.45, 0.45, 0.406_25),
        "polar_bear" => own(0.7, 0.7, 0.343_75),
        "rabbit" => own(0.24, 0.4, 0.39),
        "sheep" => own(0.45, 0.65, 0.656_25),
        "squid" | "glow_squid" => own(0.5, 0.5, 0.37),
        "turtle" => rebuilt(0.3),
        "wolf" => own(0.3, 0.425, 0.343_75),
        "strider" => own(0.45, 0.85, 0.4375),
        "hoglin" | "zoglin" => own(0.75, 0.85, 0.625),
        "piglin" | "zombified_piglin" => own(0.49, 0.98, 0.78),
        "zombie" | "drowned" => own(0.49, 0.98, 0.775),
        "husk" => own(0.49, 0.98, 0.825),
        "zombie_villager" => own(0.49, 0.98, 0.67),
        "villager" => own(0.49, 0.98, 0.63),
        _ => ty.scale(0.5),
    }
}

fn wrap(a: f32) -> f32 {
    let mut a = a % 360.0;
    if a >= 180.0 {
        a -= 360.0;
    }
    if a < -180.0 {
        a += 360.0;
    }
    a
}

/// Shortest-way angular interpolation.
pub fn rot_lerp(t: f32, a: f32, b: f32) -> f32 {
    a + t * wrap(b - a)
}

fn lerp3(t: f64, a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] + t * (b[i] - a[i]))
}

#[derive(Debug, Clone)]
struct Step {
    pos: [f64; 3],
    yaw: f32,
    pitch: f32,
    ticks: f32,
}

/// Step-by-step interpolation state (§ 4.4.3).
#[derive(Debug, Clone, Default)]
struct Stepper {
    steps: std::collections::VecDeque<Step>,
    start: Option<([f64; 3], f32, f32)>,
    t_step: f32,
    remaining: f32,
    speed: f32,
    last: Option<([f64; 3], f32, f32)>,
}

/// Metadata of an entity indexed by their index (below 32): direct read, without
/// hashing (read for each entity on every frame).
///
/// Maintained on every `insert` (the only write point): for each variant serializer present,
/// the smallest index that carries it (`RUSTCRAFT_OPT_VARIANTIDX`), and the custom name
/// (data 2) as plain text (`RUSTCRAFT_OPT_NAMECACHE`).
#[derive(Debug, Clone, Default)]
pub struct MetaMap(Vec<Option<Meta>>, Vec<(u8, u8)>, Option<Box<str>>);

/// Switch read once: `RUSTCRAFT_OPT_<name>=0` restores the old path.
fn opt_on(cell: &'static std::sync::OnceLock<bool>, value: Option<&str>) -> bool {
    *cell.get_or_init(|| value != Some("0"))
}

/// Variant read from the index held by `MetaMap` (otherwise search among all the data).
pub(crate) fn variant_idx_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    opt_on(&ON, rc_data::env_str!("RUSTCRAFT_OPT_VARIANTIDX"))
}

/// Custom name read from the plain text held by `MetaMap` (otherwise parsed on every read).
pub(crate) fn name_cache_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    opt_on(&ON, rc_data::env_str!("RUSTCRAFT_OPT_NAMECACHE"))
}

/// Custom name (data 2) as plain text, default language; `None` without a name.
pub(crate) fn parse_custom_name(m: Option<&Meta>) -> Option<String> {
    match m {
        Some(Meta::Text(Some(n))) => rc_text::from_nbt(n).map(|c| rc_text::plain_string(&c, &rc_text::Lang::default())),
        _ => None,
    }
}

impl MetaMap {
    pub fn get(&self, i: &u8) -> Option<&Meta> {
        self.0.get(usize::from(*i)).and_then(Option::as_ref)
    }

    pub fn insert(&mut self, i: u8, v: Meta) {
        let idx = usize::from(i);
        if idx >= self.0.len() {
            self.0.resize(idx + 1, None);
        }
        let was_variant = matches!(self.0[idx], Some(Meta::Variant(..)));
        let is_variant = matches!(v, Meta::Variant(..));
        self.0[idx] = Some(v);
        if was_variant || is_variant {
            // First index per serializer, in index order (like a search).
            self.1.clear();
            for (k, m) in self.0.iter().enumerate() {
                if let Some(Meta::Variant(_, s)) = m
                    && !self.1.iter().any(|(t, _)| t == s)
                {
                    self.1.push((*s, k as u8));
                }
            }
        }
        if i == 2 {
            self.2 = parse_custom_name(self.0[idx].as_ref()).map(String::into_boxed_str);
        }
    }

    pub fn values(&self) -> impl Iterator<Item = &Meta> {
        self.0.iter().flatten()
    }

    /// Variant of the serializer `ser` at the smallest index that carries it (index maintained on `insert`).
    pub fn variant_indexed(&self, ser: u8) -> Option<i32> {
        let &(_, k) = self.1.iter().find(|(s, _)| *s == ser)?;
        match self.0.get(usize::from(k)) {
            Some(Some(Meta::Variant(v, _))) => Some(*v),
            _ => None,
        }
    }

    /// Custom name as plain text, parsed on receipt.
    pub fn custom_name(&self) -> Option<&str> {
        self.2.as_deref()
    }
}

/// Cloneable: frozen copy of the appearance (item picked up in flight, `misc.rs`).
#[derive(Clone)]
pub struct Entity {
    pub kind: String,
    /// Network id of the type.
    pub kind_id: i32,
    /// Network id (witch nose, phantom flapping).
    pub id: i32,
    /// Spawn data (`add_entity`): direction of attached entities, etc.
    pub data: i32,
    pub uuid: u128,
    pub interp: Interp,
    pub class: Kind,
    /// Base of the position codec (last server position).
    pub base: [f64; 3],
    pub pos: [f64; 3],
    pub pos_o: [f64; 3],
    pub yaw: f32,
    pub yaw_o: f32,
    pub pitch: f32,
    pub pitch_o: f32,
    pub body: f32,
    pub body_o: f32,
    pub head: f32,
    pub head_o: f32,
    pub velocity: [f64; 3],
    pub on_ground: bool,
    pub meta: MetaMap,
    pub equipment: [Option<ItemStack>; 8],
    stepper: Stepper,
    linear: Option<([f64; 3], f32, f32, u32)>,
    head_target: Option<(f32, u32)>,
    /// Ticks lived on the client side.
    pub age: u32,
    pub walk_speed: f32,
    pub walk_speed_o: f32,
    pub walk_pos: f32,
    /// Cape (players): point that follows the position (current, previous), swing.
    pub cape: [f64; 3],
    pub cape_o: [f64; 3],
    pub bob: f32,
    pub bob_o: f32,
    /// Shaking of a stuck arrow (remaining ticks).
    pub shake: i32,
    /// Event timers (remaining ticks): attack (event 4), stun
    /// (39, ravager), roar; evoker fangs: remaining life once started.
    pub timers: [i32; 3],
    /// Evoker fangs started.
    pub started: bool,
    /// Free per-family render state (`entity_render::{animals, creatures, dragon}`).
    pub extra: [f32; 16],
    /// Free history (dragon: flight positions).
    pub history: Vec<[f64; 3]>,
    /// `minecraft:scale` attribute received (rendering of living entities), 1 by default.
    pub scale: f32,
    /// Distances of the name tag and of the "below name" line (attributes
    /// `name_tag_distance`, `below_name_distance` of living entities), 64 and 10 by default.
    pub tag_dist: f64,
    pub below_dist: f64,
    /// Passengers (ids, server order) and mount.
    pub passengers: Vec<i32>,
    pub vehicle: Option<i32>,
    pub hurt_time: i32,
    /// Direction of the last injury (yaw received, camera shake).
    pub hurt_dir: f32,
    pub death_time: i32,
    /// Swing: (hand, kind, duration, ticks) and animation (old, current).
    pub swing: Option<(i32, i32, i32, i32)>,
    pub swing_anim: (f32, f32),
    /// Body rotation controller of mobs: stable head, counter.
    stable_head: f32,
    head_still: i32,
    /// Sheep grazing (remaining ticks).
    pub graze: i32,
    /// Chicken flapping: (previous flap, flap, previous flap speed, flap speed, flap factor).
    pub flap: [f32; 5],
    /// Creeper swelling: (old, current).
    pub swell: (i32, i32),
    /// Physics simulated on the client (items, orbs, falling blocks, TNT: `entity_motion.rs`).
    pub motion: Option<Box<rc_physics::body::Body>>,
    /// Current shulker opening (0..1, follows data 17 by 0.05 per tick): its box.
    pub peek: f32,
    /// Fishing bobber: own state simulated on the client.
    pub bobber: Option<Box<rc_physics::body::Bobber>>,
    /// TNT minecart fuse (event 70): remaining ticks, −1 not primed.
    pub fuse: i32,
    /// Display entities: render state frozen at the tick and interpolation (`entity_render`).
    pub display: Option<Box<crate::entity_render::DisplayState>>,
    /// Poses tracked on the client (swimming, gliding, elytra, use): `entity_render`.
    pub pose_anim: crate::entity_render::PoseAnim,
    /// Box in contact with water, recorded before the tick for the kinds whose pose depends on it
    /// (`entity_render::water_track`).
    pub in_water: bool,
    /// Parrot: jukebox marked by events 1010 / 1011 and party in progress
    /// (`entity_render::party_track`).
    pub jukebox: Option<([i32; 3], bool)>,
}

impl Entity {
    pub fn flags(&self) -> i8 {
        match self.meta.get(&0) {
            Some(Meta::Byte(b)) => *b,
            _ => 0,
        }
    }

    pub fn meta_bool(&self, i: u8) -> bool {
        matches!(self.meta.get(&i), Some(Meta::Bool(true)))
    }

    pub fn meta_byte(&self, i: u8) -> i8 {
        match self.meta.get(&i) {
            Some(Meta::Byte(b)) => *b,
            _ => 0,
        }
    }

    pub fn meta_int(&self, i: u8) -> Option<i32> {
        match self.meta.get(&i) {
            Some(Meta::Int(v) | Meta::Variant(v, _) | Meta::Pose(v)) => Some(*v),
            _ => None,
        }
    }

    /// Variant carried by the serializer `ser` (cow 23, pig 28, chicken 30, cat 21,
    /// frog 27, wolf 25), whatever its index.
    pub fn variant(&self, ser: u8) -> Option<i32> {
        if variant_idx_on() {
            return self.meta.variant_indexed(ser);
        }
        self.variant_scan(ser)
    }

    /// `variant` by search among all the data (old path, test reference).
    pub fn variant_scan(&self, ser: u8) -> Option<i32> {
        self.meta.values().find_map(|m| match m {
            Meta::Variant(v, s) if *s == ser => Some(*v),
            _ => None,
        })
    }

    /// Size factor specific to the type (outside the scale attribute): cubes (slime, magma
    /// cube, sulfur cube) = synchronised size, phantom = 1 + 0.15 × size.
    pub fn size_factor(&self) -> Option<f32> {
        match self.kind.as_str() {
            "slime" | "magma_cube" | "sulfur_cube" => Some(self.meta_int(18).unwrap_or(1).clamp(1, 127) as f32),
            "phantom" => Some(1.0 + 0.15 * self.meta_int(16).unwrap_or(0).clamp(0, 64) as f32),
            _ => None,
        }
    }

    /// Current height, eye height and name tag height, outside the scale attribute
    /// (`base_size`).
    pub fn dims(&self, table: &[[f32; 4]]) -> [f32; 3] {
        let s = self.base_size(table);
        [s.h, s.eye, s.tag]
    }

    /// Dimensions of the type (`table`, by type id: width, height, eyes, name).
    fn type_size(&self, table: &[[f32; 4]]) -> Size {
        let d = usize::try_from(self.kind_id).ok().and_then(|i| table.get(i)).copied().unwrap_or([0.6, 1.8, 1.62, 1.8]);
        Size { w: d[0], h: d[1], eye: d[2], tag: d[3], fixed: false }
    }

    /// Current dimensions outside the scale attribute: pose, baby (sizes specific to each
    /// type), size of cubes and phantoms, swelling, opening, particular poses.
    pub fn base_size(&self, table: &[[f32; 4]]) -> Size {
        let living = self.class != Kind::Other;
        let pose = self.pose();
        // Sleeping: 0.2 × 0.2 fixed, for any living entity.
        if living && pose == 2 {
            return Size::fixed(0.2, 0.2).eye(0.2);
        }
        let ty = self.type_size(table);
        let float = |i: u8, d: f32| match self.meta.get(&i) {
            Some(Meta::Float(v)) => *v,
            _ => d,
        };
        match self.kind.as_str() {
            "player" | "mannequin" => {
                return match pose {
                    1 | 3 | 4 => Size::scalable(0.6, 0.6).eye(0.4),
                    5 => Size::scalable(0.6, 1.5).eye(1.27),
                    7 => Size::fixed(0.2, 0.2).eye(1.62),
                    _ => Size::scalable(0.6, 1.8).eye(1.62),
                };
            }
            // Armor stand: marker without size, small = baby.
            "armor_stand" => {
                let flags = self.meta_byte(15);
                return if flags & 0x10 != 0 {
                    Size::fixed(0.0, 0.0)
                } else if flags & 0x01 != 0 {
                    ty.scale(0.5).eye(0.9875)
                } else {
                    ty
                };
            }
            "interaction" => return Size::scalable(float(8, 1.0), float(9, 1.0)),
            "area_effect_cloud" => return Size::scalable(float(8, 3.0) * 2.0, 0.5),
            // Cubes: type × size, without age scale.
            "slime" | "magma_cube" | "sulfur_cube" => return ty.scale(self.size_factor().unwrap_or(1.0)),
            _ => {}
        }
        if !living {
            return ty;
        }
        let baby = self.baby();
        let base = if baby { baby_size(&self.kind, ty, pose) } else { ty };
        match self.kind.as_str() {
            "phantom" => base.scale(self.size_factor().unwrap_or(1.0)),
            "pufferfish" => base.scale(match self.meta_int(17).unwrap_or(0) {
                0 => 0.5,
                1 => 0.7,
                _ => 1.0,
            }),
            "salmon" => base.scale(match self.meta_int(17).unwrap_or(1) {
                0 => 0.5,
                2 => 1.5,
                _ => 1.0,
            }),
            // Open, resting on the ground: the height follows the opening.
            "shulker" if self.peek > 0.0 && matches!(self.meta.get(&16), Some(Meta::Direction(0)) | None) => {
                base.scale2(1.0, 1.0 + self.peek)
            }
            // Standing bear: client-specific animation (0 to 6).
            "polar_bear" if self.extra[0] > 0.0 => base.scale2(1.0, 1.0 + self.extra[0] / 6.0),
            "goat" if pose == 6 => base.scale(0.7),
            "camel" | "camel_husk" if pose == 10 => {
                if baby {
                    Size::scalable(0.95, 0.425).eye(0.41)
                } else {
                    Size::scalable(ty.w, ty.h - 1.43).eye(0.845)
                }
            }
            "sniffer" if self.meta_int(17) == Some(5) => {
                Size::scalable(ty.w, ty.h - 0.4).eye(0.81).scale(if baby { 0.5 } else { 1.0 })
            }
            "warden" if matches!(pose, 13 | 14) => Size::fixed(base.w, 1.0),
            _ => base,
        }
    }

    /// Standing eye height, scale attribute included (offset of the head in bed).
    pub fn standing_eye(&self, table: &[[f32; 4]]) -> f32 {
        let ty = self.type_size(table);
        let s = match self.kind.as_str() {
            "player" | "mannequin" => Size::scalable(0.6, 1.8).eye(1.62),
            _ if self.baby() => baby_size(&self.kind, ty, 0),
            _ => ty,
        };
        s.scale(self.attribute_scale()).eye
    }

    /// Scale attribute as the type accepts it (happy ghast ≤ 1, shulker ≤ 3, dragon 1).
    pub fn attribute_scale(&self) -> f32 {
        match self.kind.as_str() {
            "happy_ghast" => self.scale.min(1.0),
            "shulker" => self.scale.min(3.0),
            "ender_dragon" => 1.0,
            _ => self.scale,
        }
    }

    /// Complete current dimensions: `base_size` brought to the scale attribute (living entities).
    pub fn size(&self, table: &[[f32; 4]]) -> Size {
        self.size_from(self.base_size(table))
    }

    /// `size` from an already computed `base_size`.
    pub fn size_from(&self, s: Size) -> Size {
        if self.class != Kind::Other { s.scale(self.attribute_scale()) } else { s }
    }

    /// Attachment direction (data 8, otherwise spawn data): the game's 3D order.
    pub(crate) fn attach_dir(&self) -> i32 {
        match self.meta.get(&8) {
            Some(Meta::Direction(d)) => *d,
            _ => self.data,
        }
    }

    /// Collision box at the current position, and position recomputed by the client for
    /// attached entities (centre of the box, leash knot). `painting`: size
    /// (width, height) of a painting variant; `framed_map`: the frame holds a map.
    pub fn bounding_box(
        &self,
        table: &[[f32; 4]],
        painting: &dyn Fn(i32) -> Option<(u8, u8)>,
        framed_map: bool,
    ) -> ([f64; 3], rc_physics::Aabb) {
        let p = self.pos;
        let block = p.map(f64::floor);
        let step = |d: i32| -> [f64; 3] {
            match d {
                0 => [0.0, -1.0, 0.0],
                1 => [0.0, 1.0, 0.0],
                2 => [0.0, 0.0, -1.0],
                4 => [-1.0, 0.0, 0.0],
                5 => [1.0, 0.0, 0.0],
                _ => [0.0, 0.0, 1.0],
            }
        };
        let of_size = |c: [f64; 3], s: [f64; 3]| {
            (
                c,
                rc_physics::Aabb {
                    min: std::array::from_fn(|i| c[i] - s[i] / 2.0),
                    max: std::array::from_fn(|i| c[i] + s[i] / 2.0),
                },
            )
        };
        match self.kind.as_str() {
            "painting" => {
                let d = step(self.attach_dir());
                let (w, h) =
                    self.meta_int(9).and_then(painting).map_or((1.0, 1.0), |(w, h)| (f64::from(w), f64::from(h)));
                let off = |n: f64| if n % 2.0 == 0.0 { 0.5 } else { 0.0 };
                // Left = direction rotated by a quarter turn the opposite way.
                let left = [d[2], 0.0, -d[0]];
                let c = std::array::from_fn(|i| {
                    block[i] + 0.5 - d[i] * 0.46875 + left[i] * off(w) + if i == 1 { off(h) } else { 0.0 }
                });
                let x_axis = d[0] != 0.0;
                return of_size(c, [if x_axis { 0.0625 } else { w }, h, if x_axis { w } else { 0.0625 }]);
            }
            "item_frame" | "glow_item_frame" => {
                let d = step(self.attach_dir());
                let s = if framed_map { 1.0 } else { 0.75 };
                let c = std::array::from_fn(|i| block[i] + 0.5 - d[i] * 0.46875);
                let size = std::array::from_fn(|i| if d[i] != 0.0 { 0.0625 } else { s });
                return of_size(c, size);
            }
            "leash_knot" => {
                let ty = self.type_size(table);
                let c = [block[0] + 0.5, block[1] + 0.375, block[2] + 0.5];
                let r = f64::from(ty.w) / 2.0;
                let b = rc_physics::Aabb {
                    min: [c[0] - r, c[1], c[2] - r],
                    max: [c[0] + r, c[1] + f64::from(ty.h), c[2] + r],
                };
                return (c, b);
            }
            "shulker" => return (p, self.shulker_box()),
            _ => {}
        }
        let s = self.size(table);
        let r = f64::from(s.w) / 2.0;
        (p, rc_physics::Aabb { min: [p[0] - r, p[1], p[2] - r], max: [p[0] + r, p[1] + f64::from(s.h), p[2] + r] })
    }

    /// Shulker box: scaled cube pushed towards the face opposite the attachment, of the
    /// "physical" opening.
    pub fn shulker_box(&self) -> rc_physics::Aabb {
        let p = self.pos;
        let s = f64::from(self.attribute_scale());
        let phys = f64::from(0.5 - rc_physics::sin_t((0.5 + self.peek) * std::f32::consts::PI) * 0.5);
        let face = match self.meta.get(&16) {
            Some(Meta::Direction(v)) => *v,
            _ => 0,
        };
        // Opposite of the attachment: down → up, up → down, north → south, south → north…
        let step: [f64; 3] = match face {
            0 => [0.0, 1.0, 0.0],
            1 => [0.0, -1.0, 0.0],
            2 => [0.0, 0.0, 1.0],
            3 => [0.0, 0.0, -1.0],
            4 => [1.0, 0.0, 0.0],
            _ => [-1.0, 0.0, 0.0],
        };
        let b = rc_physics::Aabb {
            min: [p[0] - s * 0.5, p[1], p[2] - s * 0.5],
            max: [p[0] + s * 0.5, p[1] + s, p[2] + s * 0.5],
        };
        b.expand_towards(step.map(|d| d * phys * s))
    }

    /// Interpolated view rotation (yaw, pitch): head of living entities, entity rotation
    /// otherwise (frames and paintings: that of their attachment direction).
    pub fn view_rotation(&self, pt: f32) -> (f32, f32) {
        if matches!(self.kind.as_str(), "painting" | "item_frame" | "glow_item_frame") {
            // Yaw = horizontal direction × 90 (south 0, west 1, north 2, east 3); frames on the floor
            // or ceiling: pitch −90 × axis step.
            return match self.attach_dir() {
                0 => (0.0, 90.0),
                1 => (0.0, -90.0),
                2 => (180.0, 0.0),
                4 => (90.0, 0.0),
                5 => (270.0, 0.0),
                _ => (0.0, 0.0),
            };
        }
        let pitch = self.pitch_o + (self.pitch - self.pitch_o) * pt;
        if self.class != Kind::Other {
            (rot_lerp(pt, self.head_o, self.head), pitch)
        } else {
            (rot_lerp(pt, self.yaw_o, self.yaw), pitch)
        }
    }

    /// Cape angles (degrees): flap, forward tilt, sideways tilt. The
    /// distance walked is accumulated only for the local player: zero term here.
    /// `fly`: gliding opening (0..1), which clears the forward tilt.
    pub fn cape_angles(&self, pt: f32, fly: f32) -> (f32, f32, f32) {
        let pt64 = f64::from(pt);
        let lerp = |a: [f64; 3], b: [f64; 3], i: usize| a[i] + (b[i] - a[i]) * pt64;
        let d: [f64; 3] = std::array::from_fn(|i| lerp(self.cape_o, self.cape, i) - lerp(self.pos_o, self.pos, i));
        let b = f64::from(self.body_o + (self.body - self.body_o) * pt).to_radians();
        let (fx, fz) = (b.sin(), -b.cos());
        let flap = (d[1] * 10.0).clamp(-6.0, 32.0) as f32;
        let lean = ((((d[0] * fx + d[2] * fz) * 100.0) as f32) * (1.0 - fly)).clamp(0.0, 150.0);
        let lean2 = ((d[0] * fz - d[2] * fx) * 100.0).clamp(-20.0, 20.0) as f32;
        (flap, lean, lean2)
    }

    pub fn pose(&self) -> i32 {
        match self.meta.get(&6) {
            Some(Meta::Pose(p)) => *p,
            _ => 0,
        }
    }

    pub fn baby(&self) -> bool {
        self.class == Kind::Mob && self.meta_bool(if self.kind == "piglin" { 17 } else { 16 })
    }

    /// Position interpolated at render time.
    pub fn render_pos(&self, pt: f32) -> [f64; 3] {
        lerp3(f64::from(pt), self.pos_o, self.pos)
    }

    /// Target position: current interpolation target, otherwise current position.
    pub fn target_pos(&self) -> [f64; 3] {
        let stepping = self.stepper.start.is_some();
        self.linear.map(|l| l.0).or_else(|| self.stepper.last.filter(|_| stepping).map(|l| l.0)).unwrap_or(self.pos)
    }

    /// Exact placement (interpolation cancelled, old values = new ones): locally piloted vehicle
    /// repositioned by the server (`protocol-supplements.md` § 7).
    pub fn snap_to(&mut self, pos: [f64; 3], yaw: f32, pitch: f32) {
        self.set_direct(pos, yaw, pitch);
        (self.pos_o, self.yaw_o, self.pitch_o) = (self.pos, yaw, pitch);
    }

    fn set_direct(&mut self, pos: [f64; 3], yaw: f32, pitch: f32) {
        let old = self.pos;
        self.pos = pos;
        crate::entity_render::shulker_teleported(self, old);
        self.yaw = yaw;
        self.pitch = pitch;
        self.stepper = Stepper::default();
        self.linear = None;
    }

    /// New interpolation target (path and rotation).
    fn interpolate(&mut self, path: &PositionPath, yaw: f32, pitch: f32) {
        match self.interp {
            Interp::None => self.set_direct(path.end(), yaw, pitch),
            Interp::Linear(n) => {
                let n = if n == 0 { u32::try_from(self.meta_int(10).unwrap_or(0)).unwrap_or(0) } else { n };
                if n == 0 {
                    self.snap_to(path.end(), yaw, pitch);
                    return;
                }
                let target = (path.end(), yaw, pitch);
                if self.linear.is_some_and(|l| (l.0, l.1, l.2) == target) {
                    return;
                }
                self.linear = Some((target.0, target.1, target.2, n));
            }
            Interp::Stepped(n) => {
                let st = &mut self.stepper;
                let nf = n as f32;
                if st.steps.is_empty() {
                    st.start = Some((self.pos, self.yaw, self.pitch));
                    st.t_step = 1.0;
                }
                let last = st.last.unwrap_or((self.pos, self.yaw, self.pitch));
                let end = path.end();
                if end == last.0 || path.linear {
                    st.steps.push_back(Step { pos: end, yaw, pitch, ticks: nf });
                    st.remaining += nf;
                } else {
                    let total: i32 = path.steps.iter().map(|s| s.1).sum();
                    let mut acc = 0;
                    for &(p, t) in &path.steps {
                        acc += t;
                        let a = if total > 0 { acc as f32 / total as f32 } else { 1.0 };
                        let (sy, sp) = if (yaw, pitch) == (last.1, last.2) {
                            (yaw, pitch)
                        } else {
                            (rot_lerp(a, last.1, yaw), last.2 + a * (pitch - last.2))
                        };
                        st.steps.push_back(Step { pos: p, yaw: sy, pitch: sp, ticks: t.max(1) as f32 });
                        st.remaining += t.max(1) as f32;
                    }
                }
                st.last = Some((end, yaw, pitch));
                if st.speed == 0.0 {
                    st.speed = 1.0;
                }
            }
        }
    }

    /// Movement simulated by the client during a linear interpolation: the target follows
    /// (if `free` accepts the entity's box there), like the game for orbs.
    pub fn predict(&mut self, delta: [f64; 3], free: impl Fn([f64; 3]) -> bool) {
        if let Some(l) = &mut self.linear {
            let p = std::array::from_fn(|i| l.0[i] + delta[i]);
            if free(p) {
                l.0 = p;
            }
        }
    }

    fn step_interpolation(&mut self) {
        if let Some((p, yaw, pitch, n)) = self.linear {
            let a = 1.0 / n as f64;
            self.pos = lerp3(a, self.pos, p);
            self.yaw += a as f32 * wrap(yaw - self.yaw);
            self.pitch += a as f32 * (pitch - self.pitch);
            self.linear = if n > 1 { Some((p, yaw, pitch, n - 1)) } else { None };
        }
        if let Interp::Stepped(n) = self.interp {
            let st = &mut self.stepper;
            if st.start.is_none() {
                return;
            }
            // Displayed state.
            let mut state = None;
            while let Some(s) = st.steps.front() {
                if st.t_step < s.ticks {
                    let a = st.t_step / s.ticks;
                    let (p0, y0, x0) = st.start.unwrap_or((s.pos, s.yaw, s.pitch));
                    state = Some((lerp3(f64::from(a), p0, s.pos), rot_lerp(a, y0, s.yaw), x0 + a * (s.pitch - x0)));
                    break;
                }
                st.t_step -= s.ticks;
                st.start = Some((s.pos, s.yaw, s.pitch));
                st.steps.pop_front();
            }
            let (p, y, x) = state.or(st.last).unwrap_or((self.pos, self.yaw, self.pitch));
            self.pos = p;
            self.yaw = y;
            self.pitch = x;
            // Advance (catch-up if packets pile up).
            let nf = n as f32;
            let target = (st.remaining / nf).max(1.0);
            st.speed += (target - st.speed) / nf;
            let step = if st.speed < st.remaining {
                st.speed
            } else {
                st.speed = 1.0;
                st.remaining
            };
            st.t_step += step;
            st.remaining -= step;
            if st.steps.is_empty() && st.remaining <= 0.0 {
                st.start = None;
                st.remaining = 0.0;
            }
        }
    }

    /// One client tick (§ 1.1 of part 3).
    fn tick(&mut self) {
        self.age += 1;
        self.pos_o = self.pos;
        self.yaw_o = self.yaw;
        self.pitch_o = self.pitch;
        self.body_o = self.body;
        self.head_o = self.head;
        self.step_interpolation();
        crate::entity_render::display_tick(self);
        if let Some((target, n)) = self.head_target {
            self.head = rot_lerp(1.0 / n as f32, self.head, target);
            self.head_target = (n > 1).then(|| (target, n - 1));
        }
        let (dx, dz) = (self.pos[0] - self.pos_o[0], self.pos[2] - self.pos_o[2]);
        let d2 = dx * dx + dz * dz;
        match self.class {
            // Shulker: empty rotation control on the client side, body and head intact.
            Kind::Mob if self.kind == "shulker" => {}
            Kind::Other => {
                self.body = self.yaw;
                self.head = self.yaw;
            }
            Kind::Living => {
                let mut target = self.body;
                if d2 > 0.002_500_000_2 {
                    let dir = (dz.atan2(dx).to_degrees() - 90.0) as f32;
                    let diff = (wrap(self.yaw) - dir).abs();
                    target = if diff > 95.0 && diff < 265.0 { dir - 180.0 } else { dir };
                }
                if self.swing.is_some() {
                    target = self.yaw;
                }
                self.body += wrap(target - self.body) * 0.3;
                let off = wrap(self.yaw - self.body);
                if off.abs() > 50.0 {
                    self.body = self.yaw - 50.0 * off.signum();
                }
            }
            Kind::Mob => {
                let clamp_to = |base: f32, target: f32, m: f32| target - wrap(target - base).clamp(-m, m);
                if d2 > 2.5e-7 {
                    self.body = self.yaw;
                    self.head = clamp_to(self.head, self.body, 75.0);
                    self.stable_head = self.head;
                    self.head_still = 0;
                } else if (self.head - self.stable_head).abs() > 15.0 {
                    self.head_still = 0;
                    self.stable_head = self.head;
                    self.body = clamp_to(self.body, self.head, 75.0);
                } else {
                    self.head_still += 1;
                    if self.head_still > 10 {
                        let f = ((self.head_still - 10) as f32 / 10.0).clamp(0.0, 1.0);
                        self.body = clamp_to(self.body, self.head, 75.0 * (1.0 - f));
                    }
                }
            }
        }
        // Previous values within 180° of the current ones.
        self.body_o = self.body - wrap(self.body - self.body_o);
        self.head_o = self.head - wrap(self.head - self.head_o);
        self.yaw_o = self.yaw - wrap(self.yaw - self.yaw_o);
        // Limb swing.
        if self.class != Kind::Other {
            self.walk_speed_o = self.walk_speed;
            if self.death_time > 0 {
                self.walk_speed = 0.0;
                self.walk_pos = 0.0;
            } else {
                let (dist, factor) = crate::entity_render::walk_target(self, d2.sqrt() as f32);
                self.walk_speed += (dist - self.walk_speed) * factor;
                self.walk_pos += self.walk_speed;
            }
        }
        // Cape point (player and mannequin): catches up with the position by a quarter, reset
        // beyond 10 blocks. The view swing concerns only the player.
        if matches!(self.kind.as_str(), "player" | "mannequin") {
            self.cape_o = self.cape;
            for i in 0..3 {
                let d = self.pos[i] - self.cape[i];
                if d.abs() > 10.0 {
                    self.cape[i] = self.pos[i];
                    self.cape_o[i] = self.pos[i];
                } else {
                    self.cape[i] += d * 0.25;
                }
            }
        }
        if self.kind == "player" {
            self.bob_o = self.bob;
            let target = if self.on_ground && self.death_time == 0 && self.pose() != 3 {
                (d2.sqrt() as f32).min(0.1)
            } else {
                0.0
            };
            self.bob += (target - self.bob) * 0.4;
        }
        if self.shake > 0 {
            self.shake -= 1;
        }
        // TNT minecart fuse: stopped at 0 (the server makes it explode and removes it).
        if self.fuse > 0 {
            self.fuse -= 1;
        }
        crate::entity_render::tick_extra(self);
        if self.kind == "evoker_fangs" {
            if self.started {
                self.timers[0] -= 1;
            }
        } else if self.death_time == 0 {
            self.timers[0] = (self.timers[0] - 1).max(0);
            if self.timers[1] > 0 {
                self.timers[1] -= 1;
                // End of the ravager's stun: roar.
                if self.timers[1] == 0 {
                    self.timers[2] = 20;
                }
            }
            self.timers[2] = (self.timers[2] - 1).max(0);
        }
        if self.hurt_time > 0 {
            self.hurt_time -= 1;
        }
        if self.death_time > 0 {
            self.death_time += 1;
        }
        // Arm swing.
        self.swing_anim.0 = self.swing_anim.1;
        if let Some((hand, kind, duration, ticks)) = self.swing {
            self.swing_anim.1 = if duration > 0 { (ticks as f32 / duration as f32).min(1.0) } else { 1.0 };
            self.swing = (ticks < duration).then_some((hand, kind, duration, ticks + 1));
            if self.swing.is_none() {
                self.swing_anim.1 = 0.0;
            }
        } else {
            self.swing_anim.1 = 0.0;
        }
        if self.graze > 0 {
            self.graze -= 1;
        }
        if self.kind == "chicken" {
            let f = &mut self.flap;
            f[0] = f[1];
            f[2] = f[3];
            f[3] = (f[3] + if self.on_ground { -1.0 } else { 4.0 } * 0.3).clamp(0.0, 1.0);
            if !self.on_ground && f[4] < 1.0 {
                f[4] = 1.0;
            }
            f[4] *= 0.9;
            f[1] += f[4] * 2.0;
        }
        if self.kind == "creeper" {
            self.swell.0 = self.swell.1;
            // Lit (index 18): +1; otherwise swelling direction (index 16, −1 by default).
            let dir = if self.meta_bool(18) { 1 } else { self.meta_int(16).unwrap_or(-1) };
            self.swell.1 = (self.swell.1 + dir).clamp(0, 30);
        }
    }

    /// Walk animation recomputed after a movement made outside the common tick (locally
    /// piloted mount: the common tick saw it motionless).
    pub fn redo_walk(&mut self) {
        if self.class == Kind::Other || self.death_time > 0 {
            return;
        }
        self.walk_pos -= self.walk_speed;
        let (dx, dz) = (self.pos[0] - self.pos_o[0], self.pos[2] - self.pos_o[2]);
        let (dist, factor) = crate::entity_render::walk_target(self, (dx * dx + dz * dz).sqrt() as f32);
        self.walk_speed = self.walk_speed_o + (dist - self.walk_speed_o) * factor;
        self.walk_pos += self.walk_speed;
    }

    /// Hit animation at render time.
    pub fn swing_at(&self, pt: f32) -> f32 {
        let (o, n) = self.swing_anim;
        if n >= o { o + pt * (n - o) } else { o + pt * (n + 1.0 - o) }
    }

    pub fn walk_at(&self, pt: f32) -> (f32, f32) {
        let speed = (self.walk_speed_o + pt * (self.walk_speed - self.walk_speed_o)).min(1.0);
        let scale = if self.baby() { 3.0 } else { 1.0 };
        ((self.walk_pos - self.walk_speed * (1.0 - pt)) * scale, speed)
    }
}

/// Entities of the client world.
#[derive(Default)]
pub struct Entities {
    pub map: HashMap<i32, Entity>,
    /// Names of the types (`minecraft:entity_type`, without namespace).
    pub type_names: Vec<String>,
    /// Id of the local player (its movement packets move nothing).
    pub local: i32,
    /// Dimensions per type (width, height, eyes, name): eye height of the wither's targets.
    pub dims: Vec<[f32; 4]>,
    /// Local player (never announced by the server): drawn in third person, carries their
    /// metadata, injury and swing.
    pub me: Option<Entity>,
    /// Mount of the local player removed by the server (the player mounted on it, even
    /// indirectly): a `teleport_entity` that still targets it applies to the player.
    pub removed_vehicle: Option<i32>,
}

impl Entities {
    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// Position of a sound attached to entity `id`, or nothing when it must be cut (entity
    /// gone or silent, flag 4). The local player, absent from the table, follows `me`.
    pub fn sound_anchor(&self, id: i32, me: [f64; 3]) -> Option<[f64; 3]> {
        if id == self.local {
            return (!self.me.as_ref().is_some_and(|e| e.meta_bool(4))).then_some(me);
        }
        self.map.get(&id).filter(|e| !e.meta_bool(4)).map(|e| e.pos)
    }

    /// Creates the local player (kind `kind` = id of the `player` type).
    pub fn spawn_me(&mut self, uuid: u128, kind: i32, pos: [f64; 3], yaw: f32) {
        let id = self.local;
        let add = rc_proto_v777::entity::AddEntity {
            id,
            uuid,
            kind,
            pos,
            velocity: [0.0; 3],
            pitch: 0.0,
            yaw,
            head_yaw: yaw,
            data: 0,
        };
        let old = self.map.remove(&id);
        self.apply(EntityEvent::Add(Box::new(add)));
        self.me = self.map.remove(&id);
        if let Some(o) = old {
            self.map.insert(id, o);
        }
    }

    /// Respawn: new local player (default entity data: full air, no freeze,
    /// fire or stuck arrows), like the official game; their entity data is only
    /// kept if the server asks (`keep_data`, flag 2 of the packet). The server does not
    /// resend a value that went back to its default: keeping it would make the HUD lie.
    pub fn respawn_me(&mut self, keep_data: bool, pos: [f64; 3], yaw: f32) {
        let Some(old) = self.me.take() else { return };
        self.spawn_me(old.uuid, old.kind_id, pos, yaw);
        if keep_data && let Some(me) = &mut self.me {
            me.meta = old.meta;
        }
    }

    /// Local player tick: new position and rotation (reached in one tick), ground, death.
    pub fn tick_me(&mut self, pos: [f64; 3], yaw: f32, pitch: f32, on_ground: bool, dead: bool) {
        let Some(e) = &mut self.me else { return };
        // Movement of the tick (elytra, gliding in third-person view).
        e.velocity = std::array::from_fn(|i| pos[i] - e.pos[i]);
        e.linear = Some((pos, yaw, pitch, 1));
        e.head = yaw;
        e.on_ground = on_ground;
        e.death_time = if !dead { 0 } else { e.death_time.max(1) };
        e.tick();
    }

    /// Entity by id, local player included.
    fn get_mut(&mut self, id: i32) -> Option<&mut Entity> {
        if id == self.local && self.me.is_some() { self.me.as_mut() } else { self.map.get_mut(&id) }
    }

    pub fn tick(&mut self) {
        for e in self.map.values_mut() {
            e.tick();
        }
        crate::entity_render::wither_track(self);
    }

    /// Tick of these entities only (world frozen, `ticking.rs`).
    pub fn tick_some(&mut self, ids: &[i32]) {
        for id in ids {
            if let Some(e) = self.map.get_mut(id) {
                e.tick();
            }
        }
        crate::entity_render::wither_track(self);
    }

    /// Movement packet of a locally piloted vehicle: only the decoding base follows.
    pub fn update_base(&mut self, ev: &EntityEvent) {
        match ev {
            EntityEvent::Move(m) => {
                if let Some(e) = self.map.get_mut(&m.id)
                    && let Some(d) = &m.delta
                {
                    e.base = d.path(e.base).end();
                }
            }
            EntityEvent::Sync(s) => {
                if let Some(e) = self.map.get_mut(&s.id) {
                    e.base = s.path.end();
                }
            }
            _ => {}
        }
    }

    /// `rider` rides `vehicle`, directly or through other passengers (excluding `gone` entities).
    fn carries(&self, vehicle: i32, rider: i32, gone: &[i32]) -> bool {
        let mut todo = vec![vehicle];
        // Bounded: a chain of mounts does not loop, but the data comes from the network.
        for _ in 0..64 {
            let Some(v) = todo.pop() else { return false };
            let Some(e) = self.map.get(&v).filter(|_| !gone.contains(&v)) else { continue };
            if e.passengers.contains(&rider) {
                return true;
            }
            todo.extend_from_slice(&e.passengers);
        }
        false
    }

    /// Tracking of the removed mount of the local player `player`, to call BEFORE `apply`: removal
    /// of an entity they ride (even indirectly) → remembered; respawn of this
    /// id or player remounted → forgotten. Returns a `teleport_entity` targeting this absent
    /// mount: it applies to the player.
    pub fn track_player_vehicle(
        &mut self,
        ev: &EntityEvent,
        player: i32,
    ) -> Option<rc_proto_v777::entity::TeleportEntity> {
        match ev {
            // Removals in packet order: an entity already removed carries no one any more.
            EntityEvent::Remove(ids) => {
                for (i, &id) in ids.iter().enumerate() {
                    if self.carries(id, player, &ids[..i]) {
                        self.removed_vehicle = Some(id);
                    }
                }
            }
            EntityEvent::Add(a) if self.removed_vehicle == Some(a.id) => self.removed_vehicle = None,
            EntityEvent::Passengers { passengers, .. } if passengers.contains(&player) => {
                self.removed_vehicle = None;
            }
            EntityEvent::Teleport(t) if !self.map.contains_key(&t.id) && self.removed_vehicle == Some(t.id) => {
                return Some(t.clone());
            }
            _ => {}
        }
        None
    }

    pub fn apply(&mut self, ev: EntityEvent) {
        match ev {
            EntityEvent::Add(a) => {
                let kind = usize::try_from(a.kind)
                    .ok()
                    .and_then(|k| self.type_names.get(k))
                    .map(|n| n.strip_prefix("minecraft:").unwrap_or(n).to_owned())
                    .unwrap_or_default();
                let (interp, class) = classify(&kind);
                let living = class != Kind::Other;
                let (yaw, mut body) = if living { (a.yaw, a.head_yaw) } else { (a.yaw, a.yaw) };
                // Shulker: body reset to 0° on spawn.
                if kind == "shulker" {
                    body = 0.0;
                }
                let pitch = if living { a.pitch.clamp(-90.0, 90.0) } else { a.pitch };
                self.map.insert(
                    a.id,
                    Entity {
                        kind,
                        kind_id: a.kind,
                        id: a.id,
                        data: a.data,
                        uuid: a.uuid,
                        interp,
                        class,
                        base: a.pos,
                        pos: a.pos,
                        pos_o: a.pos,
                        yaw,
                        yaw_o: yaw,
                        pitch,
                        pitch_o: pitch,
                        body,
                        body_o: body,
                        head: a.head_yaw,
                        head_o: a.head_yaw,
                        velocity: a.velocity,
                        on_ground: false,
                        meta: MetaMap::default(),
                        equipment: Default::default(),
                        stepper: Stepper::default(),
                        linear: None,
                        head_target: None,
                        age: 0,
                        walk_speed: 0.0,
                        walk_speed_o: 0.0,
                        walk_pos: 0.0,
                        cape: [0.0; 3],
                        cape_o: [0.0; 3],
                        bob: 0.0,
                        bob_o: 0.0,
                        shake: 0,
                        timers: [0; 3],
                        started: false,
                        extra: [0.0; 16],
                        history: Vec::new(),
                        scale: 1.0,
                        tag_dist: 64.0,
                        below_dist: 10.0,
                        passengers: Vec::new(),
                        vehicle: None,
                        hurt_time: 0,
                        hurt_dir: 0.0,
                        death_time: 0,
                        swing: None,
                        swing_anim: (0.0, 0.0),
                        stable_head: a.head_yaw,
                        head_still: 0,
                        graze: 0,
                        flap: [0.0; 5],
                        swell: (0, 0),
                        motion: None,
                        peek: 0.0,
                        bobber: None,
                        fuse: -1,
                        display: None,
                        pose_anim: Default::default(),
                        in_water: false,
                        jukebox: None,
                    },
                );
                // Shulker: position recentred on its block from the spawn.
                if let Some(e) = self.map.get_mut(&a.id)
                    && e.kind == "shulker"
                {
                    let p = e.pos;
                    crate::entity_render::shulker_teleported(e, p);
                    e.pos_o = e.pos;
                }
            }
            EntityEvent::Remove(ids) => {
                for id in ids {
                    // Mount removed: its passengers get off (slide, body at 0).
                    let riders: Vec<i32> =
                        self.map.iter().filter(|(_, p)| p.vehicle == Some(id)).map(|(k, _)| *k).collect();
                    for r in riders {
                        if let Some(p) = self.map.get_mut(&r) {
                            p.vehicle = None;
                            crate::entity_render::shulker_riding(p, false);
                        }
                    }
                    // Passenger removed: its mount forgets it.
                    if let Some(gone) = self.map.remove(&id)
                        && let Some(v) = gone.vehicle
                        && let Some(v) = self.map.get_mut(&v)
                    {
                        v.passengers.retain(|p| *p != id);
                    }
                }
            }
            EntityEvent::Move(m) => {
                let local = m.id == self.local;
                let Some(e) = self.map.get_mut(&m.id) else { return };
                let path = m.delta.as_ref().map(|d| d.path(e.base));
                if let Some(p) = &path {
                    e.base = p.end();
                }
                if local {
                    return;
                }
                let (yaw, pitch) = m.rot.unwrap_or_else(|| e.stepper.last.map_or((e.yaw, e.pitch), |l| (l.1, l.2)));
                let path = path.unwrap_or_else(|| PositionPath {
                    linear: true,
                    steps: vec![(e.stepper.last.map_or(e.pos, |l| l.0), 0)],
                });
                crate::capseq::log_resync(m.id, e, path.end());
                e.interpolate(&path, yaw, pitch);
                e.on_ground = m.on_ground;
            }
            EntityEvent::Sync(s) => {
                let local = s.id == self.local;
                let Some(e) = self.map.get_mut(&s.id) else { return };
                e.base = s.path.end();
                if local {
                    return;
                }
                let end = s.path.end();
                crate::capseq::log_resync(s.id, e, end);
                let d: f64 = (0..3).map(|i| (end[i] - e.pos[i]).powi(2)).sum();
                if d > 4096.0 {
                    e.set_direct(end, s.yaw, s.pitch);
                    e.pos_o = e.pos;
                } else {
                    e.interpolate(&s.path, s.yaw, s.pitch);
                }
                e.on_ground = s.on_ground;
            }
            EntityEvent::Teleport(t) => {
                let Some(e) = self.map.get_mut(&t.id) else { return };
                let rel = |bit: i32| t.relative & (1 << bit) != 0;
                let cur = e.stepper.last.unwrap_or((e.pos, e.yaw, e.pitch));
                let pos: [f64; 3] = std::array::from_fn(|i| if rel(i as i32) { cur.0[i] + t.pos[i] } else { t.pos[i] });
                let yaw = if rel(3) { cur.1 + t.yaw } else { t.yaw };
                let pitch = (if rel(4) { cur.2 + t.pitch } else { t.pitch }).clamp(-90.0, 90.0);
                e.base = pos;
                crate::capseq::log_resync(t.id, e, pos);
                let d: f64 = (0..3).map(|i| (pos[i] - e.pos[i]).powi(2)).sum();
                if d > 4096.0 {
                    e.set_direct(pos, yaw, pitch);
                    e.pos_o = e.pos;
                } else {
                    e.interpolate(&PositionPath { linear: true, steps: vec![(pos, 0)] }, yaw, pitch);
                }
                e.on_ground = t.on_ground;
            }
            EntityEvent::Head { id, yaw } => {
                let Some(e) = self.map.get_mut(&id) else { return };
                if e.class == Kind::Other {
                    e.head = yaw;
                } else {
                    e.head_target = Some((yaw, 3));
                }
            }
            EntityEvent::Motion { id, velocity } => {
                if let Some(e) = self.map.get_mut(&id) {
                    e.velocity = velocity;
                    // Stuck arrow launched again (non-zero speed): it is no longer stuck.
                    let moving = velocity.iter().any(|v| *v != 0.0);
                    if moving && matches!(e.kind.as_str(), "arrow" | "spectral_arrow" | "trident") && e.meta_bool(10) {
                        e.meta.insert(10, Meta::Bool(false));
                    }
                }
            }
            EntityEvent::Data { id, values } => {
                if let Some(e) = self.get_mut(id) {
                    for (i, v) in values {
                        // Arrow that sticks: shaking of 7 ticks (part C § 4.3).
                        if i == 10
                            && matches!(e.kind.as_str(), "arrow" | "spectral_arrow")
                            && matches!(v, Meta::Bool(true))
                            && !e.meta_bool(10)
                        {
                            e.shake = 7;
                        }
                        e.meta.insert(i, v);
                        crate::entity_render::display_data(e, i);
                    }
                }
            }
            EntityEvent::Equipment { id, slots } => {
                if let Some(e) = self.map.get_mut(&id) {
                    for (s, stack) in slots {
                        if let Some(slot) = e.equipment.get_mut(s as usize) {
                            *slot = stack;
                        }
                    }
                }
            }
            EntityEvent::Event { id, event } => {
                let Some(e) = self.get_mut(id) else { return };
                if crate::entity_render::event_extra(e, event) {
                    return;
                }
                match event {
                    3 if e.class != Kind::Other && e.kind != "player" => {
                        e.death_time = e.death_time.max(1);
                    }
                    10 => e.graze = 40,
                    4 => match e.kind.as_str() {
                        "evoker_fangs" => {
                            e.started = true;
                            e.timers[0] = 22;
                        }
                        // Creaking: attack animation of 15 ticks (10 for the others).
                        "creaking" => e.timers[0] = 15,
                        _ => e.timers[0] = 10,
                    },
                    39 if e.kind == "ravager" => e.timers[1] = 40,
                    // Armor stand hit: oscillation of 5 ticks at render time.
                    32 if e.kind == "armor_stand" => e.timers[0] = 5,
                    // TNT minecart primed by the server: local fuse of 80 ticks.
                    70 if e.kind == "tnt_minecart" => e.fuse = 80,
                    // Hand swap (living entities): equipment swapped until the update.
                    55 if e.class != Kind::Other => e.equipment.swap(0, 1),
                    _ => {}
                }
            }
            EntityEvent::Swing { id, hand, kind, duration } => {
                if let Some(e) = self.get_mut(id) {
                    let busy = e.swing.is_some_and(|s| s.3 > 0 && s.3 <= s.2 / 2);
                    if !busy {
                        e.swing = Some((hand, kind, duration, 0));
                    }
                }
            }
            // Shake (`hurt_animation`): the direction changes only here. On a server, it always follows
            // a `damage_event` of the same tick (limbs already thrown); singleplayer sends
            // only it, hence the limbs thrown here too.
            EntityEvent::Hurt { id, yaw } => {
                if let Some(e) = self.get_mut(id).filter(|e| e.class != Kind::Other) {
                    e.hurt_time = 10;
                    e.hurt_dir = yaw;
                    e.walk_speed = 1.5;
                }
            }
            // Damage (`damage_event`): shake and limbs thrown, direction unchanged (the injury sound
            // of the local player is played by `entity_events.rs`).
            EntityEvent::Damage { id, .. } => {
                if let Some(e) = self.get_mut(id).filter(|e| e.class != Kind::Other) {
                    e.hurt_time = 10;
                    e.walk_speed = 1.5;
                }
            }
            EntityEvent::Passengers { vehicle, passengers } => {
                // Former passengers detached, new ones attached to the mount.
                let old = self.map.get_mut(&vehicle).map(|v| std::mem::replace(&mut v.passengers, passengers.clone()));
                for id in old.into_iter().flatten() {
                    if let Some(p) = self.map.get_mut(&id)
                        && p.vehicle == Some(vehicle)
                    {
                        p.vehicle = None;
                        crate::entity_render::shulker_riding(p, false);
                    }
                }
                if self.map.contains_key(&vehicle) {
                    for id in passengers {
                        if let Some(p) = self.map.get_mut(&id) {
                            p.vehicle = Some(vehicle);
                            crate::entity_render::shulker_riding(p, true);
                        }
                    }
                }
            }
            EntityEvent::Effect { .. } | EntityEvent::RemoveEffect { .. } | EntityEvent::Animate { .. } => {}
        }
    }
}

#[cfg(test)]
mod size_tests {
    use super::*;

    /// Entity of kind `kind` (type id 0) spawned at `pos`, spawn data `data`.
    fn spawn(kind: &str, pos: [f64; 3], data: i32) -> Entity {
        let mut all = Entities { type_names: vec![format!("minecraft:{kind}")], ..Default::default() };
        let add = rc_proto_v777::entity::AddEntity {
            id: 1,
            uuid: 1,
            kind: 0,
            pos,
            velocity: [0.0; 3],
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data,
        };
        all.apply(EntityEvent::Add(Box::new(add)));
        all.map.remove(&1).expect("spawned")
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-5
    }

    /// Mannequin cape point tracked like a player's (a quarter of the difference per tick,
    /// reset beyond 10 blocks); view swing reserved for the player.
    #[test]
    fn mannequin_cape_point_follows_like_a_player() {
        for kind in ["player", "mannequin"] {
            let mut e = spawn(kind, [4.0, 0.0, 0.0], 0);
            e.on_ground = true;
            e.tick();
            assert!(close(e.cape[0], 1.0), "{kind} : {:?}", e.cape);
            assert!(close(e.cape_o[0], 0.0), "{kind} : {:?}", e.cape_o);
            e.pos = [40.0, 0.0, 0.0];
            e.bob = 0.5;
            e.tick();
            assert!(close(e.cape[0], 40.0) && close(e.cape_o[0], 40.0), "{kind} : {:?}", e.cape);
            // Motionless: the player's swing falls back towards 0; the mannequin's does not move.
            assert_eq!(e.bob == 0.5, kind == "mannequin", "{kind} : {}", e.bob);
        }
    }

    /// Attached sound (goat horn of the local player, `sound_entity`): the local player, absent from the
    /// table, is tracked at their current position; a known entity at its own; an entity
    /// gone or silent cuts the sound.
    #[test]
    fn attached_sounds_follow_their_entity() {
        let mut all = Entities { local: 7, ..Default::default() };
        assert_eq!(all.sound_anchor(7, [200.0, 70.0, 0.0]), Some([200.0, 70.0, 0.0]));
        all.me = Some(spawn("player", [0.0; 3], 0));
        assert_eq!(all.sound_anchor(7, [5.0, 6.0, 7.0]), Some([5.0, 6.0, 7.0]));
        all.me.as_mut().expect("player").meta.insert(4, rc_proto_v777::entity::Meta::Bool(true));
        assert_eq!(all.sound_anchor(7, [5.0, 6.0, 7.0]), None);
        all.map.insert(3, spawn("pig", [1.0, 2.0, 3.0], 0));
        assert_eq!(all.sound_anchor(3, [5.0, 6.0, 7.0]), Some([1.0, 2.0, 3.0]));
        assert_eq!(all.sound_anchor(4, [5.0, 6.0, 7.0]), None);
    }

    /// Mount removed under the player (9, by a horse 2 on a boat 1): the teleport of the removed
    /// boat goes back to the player; forgotten as soon as the player remounts or the id returns.
    #[test]
    fn teleport_of_removed_vehicle_goes_to_player() {
        let mut all = Entities { type_names: vec!["minecraft:oak_boat".into()], ..Default::default() };
        for id in [1, 2] {
            let add = rc_proto_v777::entity::AddEntity {
                id,
                uuid: id as u128,
                kind: 0,
                pos: [0.0; 3],
                velocity: [0.0; 3],
                pitch: 0.0,
                yaw: 0.0,
                head_yaw: 0.0,
                data: 0,
            };
            all.apply(EntityEvent::Add(Box::new(add)));
        }
        all.apply(EntityEvent::Passengers { vehicle: 1, passengers: vec![2] });
        all.apply(EntityEvent::Passengers { vehicle: 2, passengers: vec![9] });
        let tp = rc_proto_v777::entity::TeleportEntity {
            id: 1,
            pos: [5.0, 70.0, 5.0],
            velocity: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            relative: 0,
            on_ground: false,
        };
        let remove = EntityEvent::Remove(vec![1]);
        assert_eq!(all.track_player_vehicle(&remove, 9), None);
        all.apply(remove);
        assert_eq!(all.removed_vehicle, Some(1));
        let ev = EntityEvent::Teleport(tp.clone());
        assert_eq!(all.track_player_vehicle(&ev, 9), Some(tp.clone()));
        // The player remounts elsewhere: nothing left to forward.
        all.track_player_vehicle(&EntityEvent::Passengers { vehicle: 2, passengers: vec![9] }, 9);
        assert_eq!(all.track_player_vehicle(&ev, 9), None);
        // Mount that does not carry the player: ignored.
        assert_eq!(all.track_player_vehicle(&EntityEvent::Remove(vec![5]), 9), None);
        assert_eq!(all.removed_vehicle, None);
    }

    #[test]
    fn respawn_gives_a_fresh_local_player_unless_data_is_kept() {
        let mut all = Entities { type_names: vec!["minecraft:player".into()], local: 7, ..Default::default() };
        all.spawn_me(42, 0, [0.0; 3], 0.0);
        let dirty = |all: &mut Entities| {
            let me = all.me.as_mut().unwrap();
            me.meta.insert(1, Meta::Int(98)); // air at half breath
            me.meta.insert(7, Meta::Int(140)); // frozen
        };
        dirty(&mut all);
        all.respawn_me(false, [1.0, 2.0, 3.0], 90.0);
        let me = all.me.as_ref().unwrap();
        assert_eq!((me.uuid, me.kind_id, me.meta_int(1), me.meta_int(7)), (42, 0, None, None));
        dirty(&mut all);
        all.respawn_me(true, [0.0; 3], 0.0);
        assert_eq!(all.me.as_ref().unwrap().meta_int(1), Some(98));
        assert!(all.map.is_empty());
    }

    #[test]
    fn cube_size_scales_width_too() {
        let mut e = spawn("slime", [0.0; 3], 0);
        e.meta.insert(18, Meta::Int(4));
        let s = e.size(&[[0.52, 0.52, 0.325, 0.52]]);
        assert!((s.w - 2.08).abs() < 1e-5 && (s.h - 2.08).abs() < 1e-5, "{s:?}");
    }

    #[test]
    fn babies_use_their_own_dimensions() {
        let mut e = spawn("zombie", [0.0; 3], 0);
        e.meta.insert(16, Meta::Bool(true));
        let s = e.size(&[[0.6, 1.95, 1.74, 1.95]]);
        assert_eq!((s.w, s.h, s.eye), (0.49, 0.98, 0.775));
        let mut c = spawn("cow", [0.0; 3], 0);
        c.meta.insert(16, Meta::Bool(true));
        let s = c.size(&[[0.9, 1.4, 1.3, 1.4]]);
        assert_eq!((s.w, s.h, s.eye), (0.45, 0.7, 0.69));
        // Without its own size: type × 1/2.
        let mut b = spawn("bee", [0.0; 3], 0);
        b.meta.insert(16, Meta::Bool(true));
        assert_eq!(b.size(&[[0.7, 0.6, 0.3, 0.6]]).w, 0.35);
    }

    #[test]
    fn scale_attribute_but_not_when_asleep() {
        let mut e = spawn("pig", [0.0; 3], 0);
        e.scale = 2.0;
        let t = [[0.9, 0.9, 0.765, 0.9]];
        assert_eq!(e.size(&t).w, 1.8);
        e.meta.insert(6, Meta::Pose(2));
        assert_eq!(e.size(&t).w, 0.2);
    }

    #[test]
    fn interaction_box_from_its_data() {
        let mut e = spawn("interaction", [1.0, 2.0, 3.0], 0);
        e.meta.insert(8, Meta::Float(2.5));
        e.meta.insert(9, Meta::Float(0.5));
        let (_, b) = e.bounding_box(&[], &|_| None, false);
        assert!(close(b.min[0], -0.25) && close(b.max[0], 2.25) && close(b.max[1], 2.5), "{b:?}");
    }

    #[test]
    fn painting_box_follows_variant_and_facing() {
        // 4 × 3 attached to the north wall of the block (10, 64, −5): facing north (data 2).
        let mut e = spawn("painting", [10.0, 64.0, -5.0], 2);
        e.meta.insert(9, Meta::Int(0));
        let (c, b) = e.bounding_box(&[], &|v| (v == 0).then_some((4, 3)), false);
        assert!(close(c[0], 10.0) && close(c[1], 64.5) && close(c[2], -4.031_25), "{c:?}");
        assert!(close(b.max[0] - b.min[0], 4.0) && close(b.max[1] - b.min[1], 3.0));
        assert!(close(b.max[2] - b.min[2], 0.0625));
        assert_eq!(e.view_rotation(0.5), (180.0, 0.0));
    }

    #[test]
    fn item_frame_box_on_floor_and_with_map() {
        let e = spawn("item_frame", [3.0, 70.0, 3.0], 1);
        let (c, b) = e.bounding_box(&[], &|_| None, false);
        assert!(close(c[1], 70.031_25) && close(b.max[1] - b.min[1], 0.0625) && close(b.max[0] - b.min[0], 0.75));
        let (_, m) = e.bounding_box(&[], &|_| None, true);
        assert!(close(m.max[0] - m.min[0], 1.0));
        assert_eq!(e.view_rotation(0.0), (0.0, -90.0));
    }

    #[test]
    fn display_distance_follows_cube_size() {
        let table = [[0.52, 0.52, 0.325, 0.52]];
        let mut e = spawn("slime", [0.0; 3], 0);
        e.meta.insert(18, Meta::Int(4));
        // Mean side 2.08 × 64 = 133 blocks, not 33.
        assert!(crate::entity_render::within_render_distance(&e, 100.0 * 100.0, 1.0, &table));
        assert!(!crate::entity_render::within_render_distance(&e, 140.0 * 140.0, 1.0, &table));
    }

    #[test]
    fn baby_zombie_vehicle_attach() {
        let mut e = spawn("zombie", [0.0; 3], 0);
        assert!(close(crate::riding::rider_attach(&e, 1.0), 0.7));
        e.meta.insert(16, Meta::Bool(true));
        assert!(close(crate::riding::rider_attach(&e, 1.0), 0.1875));
        assert!(close(crate::riding::rider_attach(&e, 2.0), 0.375));
        let mut v = spawn("zombie_villager", [0.0; 3], 0);
        v.meta.insert(16, Meta::Bool(true));
        assert!(close(crate::riding::rider_attach(&v, 1.0), 0.125));
    }

    #[test]
    fn marker_armor_stand_has_no_size() {
        let mut e = spawn("armor_stand", [0.0; 3], 0);
        e.meta.insert(15, Meta::Byte(0x10));
        let s = e.size(&[[0.5, 1.975, 1.7775, 1.975]]);
        assert_eq!((s.w, s.h), (0.0, 0.0));
        e.meta.insert(15, Meta::Byte(0x01));
        let s = e.size(&[[0.5, 1.975, 1.7775, 1.975]]);
        assert_eq!((s.w, s.eye), (0.25, 0.9875));
    }

    #[test]
    fn shulker_box_scales_and_opens() {
        let mut e = spawn("shulker", [0.0, 64.0, 0.0], 0);
        e.scale = 2.0;
        e.peek = 1.0;
        let b = e.shulker_box();
        // Scale 2: cube of 2, open upwards by 2 × physical opening (1 → 1.0).
        assert!(close(b.max[0] - b.min[0], 2.0) && close(b.max[1], 64.0 + 2.0 + 2.0), "{b:?}");
    }
}
