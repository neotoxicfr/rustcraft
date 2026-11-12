//! Client-side entities (`docs/specs/entities.md` part 1 § 4, part 3 § 1): received state,
//! position interpolation, body rotation, limb swing, hits and death.

use hashbrown::HashMap;

use rc_client::EntityEvent;
use rc_data::java_math::wrap_degrees;
use rc_proto_v777::entity::{Meta, PositionPath};
use rc_proto_v777::item::ItemStack;

mod kind_facts;
mod size_rules;
pub(crate) mod sizes_by_name;
mod subsets;
pub(crate) use kind_facts::{KindFacts, MotionFamily, PickRule, kind_flags_on, kind_id_filters_on};
pub(crate) use size_rules::{DistanceRule, ScaleBound, SizeClass, SizeRules, kind_size_rules_on};
pub use subsets::EntitySubsets;
pub(crate) use subsets::{entity_subsets_on, rare_kind_lists_on};

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

/// "Physical" opening of a shulker for an opening `peek` in 0..1: 0.5 − sin((0.5 + peek)·π)·0.5.
pub fn shulker_physical(peek: f32) -> f32 {
    0.5 - rc_physics::sin_t((0.5 + peek) * std::f32::consts::PI) * 0.5
}

/// Shortest-way angular interpolation in degrees (the shared fold of the game, which skips the
/// library remainder below one turn).
#[inline]
pub fn rot_lerp(t: f32, a: f32, b: f32) -> f32 {
    rc_data::java_math::lerp_angle_degrees(t, a, b)
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
/// the smallest index that carries it (`RUSTCRAFT_OPT_VARIANTIDX`), the custom name
/// (data 2) as plain text (`RUSTCRAFT_OPT_NAMECACHE`), and a stamp that changes on every write.
#[derive(Debug, Clone, Default)]
pub struct MetaMap(Vec<Option<Meta>>, Vec<(u8, u8)>, Option<Box<str>>, u64);

/// Next write stamp of a `MetaMap`. Shared by every map so that a map replaced as a whole never
/// shows the stamp of the one it replaces (0 only for a map never written).
fn next_meta_stamp() -> u64 {
    static STAMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    STAMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Variant read from the index held by `MetaMap` (otherwise search among all the data).
#[inline]
pub(crate) fn variant_idx_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_VARIANTIDX")
}

/// Custom name read from the plain text held by `MetaMap` (otherwise parsed on every read).
#[inline]
pub(crate) fn name_cache_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_NAMECACHE")
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
        self.3 = next_meta_stamp();
    }

    /// Changes on every `insert`: what was computed from the data at one stamp still holds while
    /// the stamp stays the same.
    pub fn stamp(&self) -> u64 {
        self.3
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

/// What an entity's box is computed from, apart from its type (fixed at spawn): position, data
/// write stamp, scale attribute, dimension table (address and length).
#[derive(Clone, Copy, PartialEq, Eq)]
struct BoxKey {
    pos: [u64; 3],
    meta: u64,
    scale: u32,
    table: (usize, usize),
}

#[derive(Clone, Copy)]
struct CachedBox {
    key: BoxKey,
    bb: rc_physics::Aabb,
}

#[cfg(test)]
thread_local! {
    /// Sabotage for the proof of the box cache: the key forgets the scale attribute, so a box
    /// computed before a scale change is read back after it. The proof must catch it.
    pub(crate) static BOX_KEY_FORGETS_SCALE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Scale attribute as written in a box key.
fn box_key_scale(scale: f32) -> u32 {
    #[cfg(test)]
    if BOX_KEY_FORGETS_SCALE.get() {
        return 0;
    }
    scale.to_bits()
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
    /// Facts of the type, worked out from `kind` at spawn (`kind_facts.rs`).
    pub facts: KindFacts,
    /// Box of the last `cached_box` call and what it was computed from.
    box_cache: std::cell::Cell<Option<CachedBox>>,
    /// Attributes synchronised by the server (`update_attributes`): id in the
    /// `minecraft:attribute` registry and current value. They go with the entity: a new entity
    /// that reuses the id after a world change starts from its type's defaults, like the game.
    pub attributes: Vec<(u16, f64)>,
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
    /// Body rotation controller of mobs: stable head, counter, and "first passenger is a mob"
    /// (refreshed before each tick: the body then keeps its rotation while standing still).
    stable_head: f32,
    head_still: i32,
    carries_mob: bool,
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
    /// Counters of the client effects per kind (`entity_fx`): enderman 0 = next tick at which the
    /// stare sound is allowed; breeze 0 whistle, 1 jump trail, 2 ambient sound;
    /// ravager 0 stun seen at the previous tick.
    pub fx: [i32; 3],
    /// Touched the water at the previous splash check (`entity_fx`); `None` before the first one
    /// (no splash on the first tick, like the game) and while the entity is not checked.
    pub was_wet: Option<bool>,
}

impl Entity {
    /// Value of the synchronised attribute named `name` (`names`: the `minecraft:attribute`
    /// registry), `None` if the server never sent it.
    pub fn synced_attribute(&self, names: &[String], name: &str) -> Option<f64> {
        let named = |id: u16| names.get(usize::from(id)).is_some_and(|n| n == name);
        self.attributes.iter().find(|(id, _)| named(*id)).map(|(_, v)| *v)
    }

    /// Stores the synchronised attribute `id` (replacing its previous value).
    pub fn set_synced_attribute(&mut self, id: u16, value: f64) {
        match self.attributes.iter_mut().find(|(a, _)| *a == id) {
            Some(slot) => slot.1 = value,
            None => self.attributes.push((id, value)),
        }
    }

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
        if kind_size_rules_on() { self.size_factor_by_rules() } else { sizes_by_name::size_factor(self) }
    }

    /// `size_factor` from the rules stored at spawn.
    fn size_factor_by_rules(&self) -> Option<f32> {
        match self.facts.size.class {
            SizeClass::Cube => Some(self.meta_int(18).unwrap_or(1).clamp(1, 127) as f32),
            SizeClass::Phantom => Some(1.0 + 0.15 * self.meta_int(16).unwrap_or(0).clamp(0, 64) as f32),
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
    // Never inlined: the size then leaves as one block that the caller copies whole. Inlined into
    // `base_size_by_rules`, it was split into separate fields, written one by one and read back as
    // a block by whoever used the size next, a reload the processor cannot serve from its pending
    // writes: about 8 ns per entity (`bench_size_rules`, the `crowd-1000` animals 16.5 ns, 9 ns
    // with this). The old path keeps its own inlined copy (`sizes_by_name::type_size`).
    #[inline(never)]
    fn type_size(&self, table: &[[f32; 4]]) -> Size {
        let d = usize::try_from(self.kind_id).ok().and_then(|i| table.get(i)).copied().unwrap_or([0.6, 1.8, 1.62, 1.8]);
        Size { w: d[0], h: d[1], eye: d[2], tag: d[3], fixed: false }
    }

    /// Current dimensions outside the scale attribute: pose, baby (sizes specific to each
    /// type), size of cubes and phantoms, swelling, opening, particular poses.
    pub fn base_size(&self, table: &[[f32; 4]]) -> Size {
        if kind_size_rules_on() { self.base_size_by_rules(table) } else { sizes_by_name::base_size(self, table) }
    }

    /// `base_size` from the rules stored at spawn.
    pub(crate) fn base_size_by_rules(&self, table: &[[f32; 4]]) -> Size {
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
        let rules = &self.facts.size;
        match rules.class {
            SizeClass::Avatar => {
                return match pose {
                    1 | 3 | 4 => Size::scalable(0.6, 0.6).eye(0.4),
                    5 => Size::scalable(0.6, 1.5).eye(1.27),
                    7 => Size::fixed(0.2, 0.2).eye(1.62),
                    _ => Size::scalable(0.6, 1.8).eye(1.62),
                };
            }
            // Armor stand: marker without size, small = baby.
            SizeClass::ArmorStand => {
                let flags = self.meta_byte(15);
                return if flags & 0x10 != 0 {
                    Size::fixed(0.0, 0.0)
                } else if flags & 0x01 != 0 {
                    ty.scale(0.5).eye(0.9875)
                } else {
                    ty
                };
            }
            SizeClass::Interaction => return Size::scalable(float(8, 1.0), float(9, 1.0)),
            SizeClass::EffectCloud => return Size::scalable(float(8, 3.0) * 2.0, 0.5),
            // Cubes: type × size, without age scale.
            SizeClass::Cube => return ty.scale(self.size_factor_by_rules().unwrap_or(1.0)),
            _ => {}
        }
        if !living {
            return ty;
        }
        let baby = self.baby_by_rules();
        let base = if baby { rules.baby_size().size(ty, pose) } else { ty };
        match rules.class {
            SizeClass::Phantom => base.scale(self.size_factor_by_rules().unwrap_or(1.0)),
            SizeClass::Pufferfish => base.scale(match self.meta_int(17).unwrap_or(0) {
                0 => 0.5,
                1 => 0.7,
                _ => 1.0,
            }),
            SizeClass::Salmon => base.scale(match self.meta_int(17).unwrap_or(1) {
                0 => 0.5,
                2 => 1.5,
                _ => 1.0,
            }),
            // Open, resting on the ground: the height follows the opening.
            SizeClass::Shulker
                if self.peek > 0.0 && matches!(self.meta.get(&16), Some(Meta::Direction(0)) | None) =>
            {
                base.scale2(1.0, 1.0 + self.peek)
            }
            // Standing bear: client-specific animation (0 to 6).
            SizeClass::PolarBear if self.extra[0] > 0.0 => base.scale2(1.0, 1.0 + self.extra[0] / 6.0),
            SizeClass::Goat if pose == 6 => base.scale(0.7),
            SizeClass::Camel if pose == 10 => {
                if baby {
                    Size::scalable(0.95, 0.425).eye(0.41)
                } else {
                    Size::scalable(ty.w, ty.h - 1.43).eye(0.845)
                }
            }
            // Digging (state 5 of entity data 18; 17 is the age lock): 0.4 lower.
            SizeClass::Sniffer if self.meta_int(18) == Some(5) => {
                Size::scalable(ty.w, ty.h - 0.4).eye(0.81).scale(if baby { 0.5 } else { 1.0 })
            }
            SizeClass::Warden if matches!(pose, 13 | 14) => Size::fixed(base.w, 1.0),
            _ => base,
        }
    }

    /// Standing eye height, scale attribute included (offset of the head in bed).
    pub fn standing_eye(&self, table: &[[f32; 4]]) -> f32 {
        if !kind_size_rules_on() {
            return sizes_by_name::standing_eye(self, table);
        }
        let ty = self.type_size(table);
        let s = match self.facts.size.class {
            SizeClass::Avatar => Size::scalable(0.6, 1.8).eye(1.62),
            _ if self.baby_by_rules() => self.facts.size.baby_size().size(ty, 0),
            _ => ty,
        };
        s.scale(self.attribute_scale_by_rules()).eye
    }

    /// Scale attribute as the type accepts it (happy ghast ≤ 1, shulker ≤ 3, dragon 1).
    pub fn attribute_scale(&self) -> f32 {
        if kind_size_rules_on() { self.attribute_scale_by_rules() } else { sizes_by_name::attribute_scale(self) }
    }

    /// `attribute_scale` from the rules stored at spawn.
    pub(crate) fn attribute_scale_by_rules(&self) -> f32 {
        match self.facts.size.scale_bound {
            ScaleBound::AtMostOne => self.scale.min(1.0),
            ScaleBound::AtMostThree => self.scale.min(3.0),
            ScaleBound::AlwaysOne => 1.0,
            ScaleBound::Free => self.scale,
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

    /// `bounding_box(..).1`, kept from the previous call while its position, data, scale and
    /// dimension table stay the same (`RUSTCRAFT_OPT_CACHED_ENTITY_BOXES`): between two ticks
    /// nothing moves, so every frame after the first one of a tick reads the box back.
    /// `framed_map` is only asked for item frames, and only when the box is computed.
    pub fn cached_box(
        &self,
        table: &[[f32; 4]],
        painting: &dyn Fn(i32) -> Option<(u8, u8)>,
        framed_map: &dyn Fn(&Entity) -> bool,
    ) -> rc_physics::Aabb {
        let compute = || self.bounding_box(table, painting, self.facts.item_frame && framed_map(self)).1;
        if !self.facts.box_cacheable {
            return compute();
        }
        let key = BoxKey {
            pos: self.pos.map(f64::to_bits),
            meta: self.meta.stamp(),
            scale: box_key_scale(self.scale),
            table: (table.as_ptr() as usize, table.len()),
        };
        if let Some(c) = self.box_cache.get()
            && c.key == key
        {
            return c.bb;
        }
        let bb = compute();
        self.box_cache.set(Some(CachedBox { key, bb }));
        bb
    }

    /// Shulker box: scaled cube pushed towards the face opposite the attachment, of the
    /// "physical" opening.
    pub fn shulker_box(&self) -> rc_physics::Aabb {
        self.shulker_zone(-1.0, shulker_physical(self.peek))
    }

    /// Opening direction of a shulker (opposite of its attachment face), as a unit step.
    pub fn shulker_step(&self) -> [f64; 3] {
        let face = match self.meta.get(&16) {
            Some(Meta::Direction(v)) => *v,
            _ => 0,
        };
        // Opposite of the attachment: down → up, up → down, north → south, south → north…
        match face {
            0 => [0.0, 1.0, 0.0],
            1 => [0.0, -1.0, 0.0],
            2 => [0.0, 0.0, 1.0],
            3 => [0.0, 0.0, -1.0],
            4 => [1.0, 0.0, 0.0],
            _ => [-1.0, 0.0, 0.0],
        }
    }

    /// Part of the shulker's scaled cube between the physical openings `from` and `to`, toward
    /// its opening direction (`from` = −1: the whole box at opening `to`; otherwise the slice
    /// that an opening sweeps).
    pub fn shulker_zone(&self, from: f32, to: f32) -> rc_physics::Aabb {
        let p = self.pos;
        let s = f64::from(self.attribute_scale());
        let step = self.shulker_step();
        let (hi, lo) = (f64::from(from.max(to)), f64::from(from.min(to)));
        let mut b = rc_physics::Aabb {
            min: [p[0] - s * 0.5, p[1], p[2] - s * 0.5],
            max: [p[0] + s * 0.5, p[1] + s, p[2] + s * 0.5],
        }
        .expand_towards(step.map(|d| d * hi * s));
        // Then shrunk from the attachment side by (1 + lo) sizes.
        for (k, d) in step.into_iter().enumerate() {
            let c = -d * (1.0 + lo) * s;
            if c < 0.0 {
                b.min[k] -= c;
            } else if c > 0.0 {
                b.max[k] -= c;
            }
        }
        b
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

    /// Another type name and the facts that go with it, for a test that changes an entity's kind
    /// by hand (the network never does: a new type is a new entity).
    #[cfg(test)]
    pub(crate) fn retype(&mut self, kind: &str) {
        kind.clone_into(&mut self.kind);
        self.facts = KindFacts::of(kind);
    }

    pub fn baby(&self) -> bool {
        if kind_size_rules_on() { self.baby_by_rules() } else { sizes_by_name::baby(self) }
    }

    /// `baby` from the rules stored at spawn.
    pub(crate) fn baby_by_rules(&self) -> bool {
        self.class == Kind::Mob && self.meta_bool(self.facts.size.baby_flag_index)
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
            self.yaw += a as f32 * wrap_degrees(yaw - self.yaw);
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
                    let diff = (wrap_degrees(self.yaw) - dir).abs();
                    target = if diff > 95.0 && diff < 265.0 { dir - 180.0 } else { dir };
                }
                if self.swing.is_some() {
                    target = self.yaw;
                }
                self.body += wrap_degrees(target - self.body) * 0.3;
                let off = wrap_degrees(self.yaw - self.body);
                if off.abs() > 50.0 {
                    self.body = self.yaw - 50.0 * off.signum();
                }
            }
            Kind::Mob => {
                let clamp_to = |base: f32, target: f32, m: f32| target - wrap_degrees(target - base).clamp(-m, m);
                if d2 > 2.5e-7 {
                    self.body = self.yaw;
                    self.head = clamp_to(self.head, self.body, 75.0);
                    self.stable_head = self.head;
                    self.head_still = 0;
                } else if self.carries_mob {
                    // Standing still with a mob as first passenger: the body keeps its rotation.
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
        self.body_o = self.body - wrap_degrees(self.body - self.body_o);
        self.head_o = self.head - wrap_degrees(self.head - self.head_o);
        self.yaw_o = self.yaw - wrap_degrees(self.yaw - self.yaw_o);
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
    /// Local sounds born from a metadata update (enderman stare), played by the caller right
    /// after `apply`.
    pub pending_sounds: Vec<crate::entity_fx::FxSound>,
    /// Vehicles that received passengers: the mob-rider pass of a tick walks only them.
    pub riders: Vec<i32>,
    /// Rare subsets (glowing, invisible, name-tag candidates, rare kinds) kept by `apply`.
    pub subsets: EntitySubsets,
    /// The local player touched the water at the previous splash check (`Entity::was_wet`).
    pub local_was_wet: Option<bool>,
}

/// `RUSTCRAFT_OPT_RIDERLIST=0`: the mob-rider pass walks every entity instead of the vehicles.
pub(crate) fn rider_list_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_RIDERLIST")
}

impl Entities {
    pub fn clear(&mut self) {
        self.map.clear();
        self.riders.clear();
        self.subsets.clear();
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
        // The game keeps last tick's head yaw as the previous one, then turns the head to the
        // player's yaw: set it before the tick (which reads it), but restore the old value as
        // the previous one afterwards, or the head would not interpolate between ticks.
        let head_before = e.head;
        e.head = yaw;
        e.on_ground = on_ground;
        e.death_time = if !dead { 0 } else { e.death_time.max(1) };
        e.tick();
        e.head_o = e.head - wrap_degrees(e.head - head_before);
    }

    /// Entity by id, local player included.
    fn get_mut(&mut self, id: i32) -> Option<&mut Entity> {
        if id == self.local && self.me.is_some() { self.me.as_mut() } else { self.map.get_mut(&id) }
    }

    pub fn tick(&mut self) {
        self.mark_mob_riders();
        for e in self.map.values_mut() {
            e.tick();
        }
        crate::entity_render::wither_track(self);
        self.subsets.drop_stale(&self.map);
    }

    /// Entities whose first passenger is a mob (the body rotation of a standing mob depends on it).
    fn mark_mob_riders(&mut self) {
        self.mark_mob_riders_with(rider_list_on());
    }

    fn mark_mob_riders_with(&mut self, list: bool) {
        let carries = |e: &Entity| e.passengers.first().and_then(|p| self.map.get(p)).is_some_and(|p| p.class == Kind::Mob);
        let carried: Vec<(i32, bool)> = if list {
            // Vehicles only; those gone or without passengers leave the list once reset.
            self.riders.iter().filter_map(|id| self.map.get(id).map(|e| (*id, carries(e)))).collect()
        } else {
            let walk = self.map.iter().filter(|(_, e)| e.carries_mob || !e.passengers.is_empty());
            walk.map(|(&id, e)| (id, carries(e))).collect()
        };
        for &(id, v) in &carried {
            if let Some(e) = self.map.get_mut(&id) {
                e.carries_mob = v;
            }
        }
        let map = &self.map;
        self.riders.retain(|id| map.get(id).is_some_and(|e| !e.passengers.is_empty()));
    }

    /// Tick of these entities only (world frozen, `ticking.rs`).
    pub fn tick_some(&mut self, ids: &[i32]) {
        self.mark_mob_riders();
        for id in ids {
            if let Some(e) = self.map.get_mut(id) {
                e.tick();
            }
        }
        crate::entity_render::wither_track(self);
        self.subsets.drop_stale(&self.map);
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

    /// Vehicle that counts `rider` among its passengers: looked for among the vehicles of
    /// `riders` (`from_list`), or among every entity (old path). A rider is listed by one
    /// vehicle at most (`leave_previous_vehicles`), so both find the same one.
    pub fn vehicle_carrying(&self, rider: i32, from_list: bool) -> Option<i32> {
        if from_list {
            let carries = |v: &i32| self.map.get(v).is_some_and(|e| e.passengers.contains(&rider));
            self.riders.iter().copied().find(carries)
        } else {
            self.map.iter().find(|(_, e)| e.passengers.contains(&rider)).map(|(id, _)| *id)
        }
    }

    /// Root vehicle of `id`: the one up its chain of vehicles that rides nothing (8 levels at most).
    pub fn root_vehicle_of(&self, mut id: i32, from_list: bool) -> i32 {
        for _ in 0..8 {
            match self.vehicle_carrying(id, from_list) {
                Some(v) => id = v,
                None => break,
            }
        }
        id
    }

    /// Vehicles whose passengers are seated first: those with passengers that ride no known
    /// entity. From the vehicles of `riders` (`from_list`), or from every entity (old path).
    pub fn seat_roots(&self, from_list: bool) -> Vec<i32> {
        let root = |e: &Entity| !e.passengers.is_empty() && e.vehicle.is_none_or(|v| !self.map.contains_key(&v));
        if from_list {
            self.riders.iter().copied().filter(|id| self.map.get(id).is_some_and(root)).collect()
        } else {
            self.map.iter().filter(|(_, e)| root(e)).map(|(id, _)| *id).collect()
        }
    }

    /// Passengers announced on `vehicle` leave the vehicle that listed them before, like the game,
    /// which dismounts an entity before mounting it elsewhere. Otherwise, until the old vehicle's
    /// own update, a passenger moved from one vehicle to another stays listed by both. Nothing
    /// moves for an unknown vehicle (the game ignores the packet).
    fn leave_previous_vehicles(&mut self, vehicle: i32, passengers: &[i32]) {
        if !self.map.contains_key(&vehicle) {
            return;
        }
        for &p in passengers {
            let previous = if p == self.local {
                self.vehicle_carrying(p, rider_list_on())
            } else {
                self.map.get(&p).and_then(|e| e.vehicle)
            };
            if let Some(old) = previous.filter(|&old| old != vehicle)
                && let Some(v) = self.map.get_mut(&old)
            {
                v.passengers.retain(|&x| x != p);
            }
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
                // An id already in use: the old entity is removed first, like the game (its
                // passengers get off, its vehicle forgets it).
                if self.map.contains_key(&a.id) {
                    self.apply(EntityEvent::Remove(vec![a.id]));
                }
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
                let facts = KindFacts::of(&kind);
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
                        facts,
                        box_cache: std::cell::Cell::new(None),
                        attributes: Vec::new(),
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
                        carries_mob: false,
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
                        fx: [0; 3],
                        was_wet: None,
                    },
                );
                if let Some(e) = self.map.get(&a.id) {
                    self.subsets.note_spawn(a.id, e);
                }
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
                } else if e.kind != "ender_dragon" {
                    // The dragon keeps the target without ever smoothing it (its AI step replaces the
                    // living entity's, which does it): its head only follows the body catch-up.
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
                let mut stare = false;
                // Flags (0), visible name (3) and effect particles (10) decide the rare subsets.
                let subset_data = values.iter().any(|(i, _)| EntitySubsets::is_subset_data(*i));
                if let Some(e) = self.get_mut(id) {
                    for (i, v) in values {
                        // Enderman: data 17 (threatening) received while data 18 (stared at) is already
                        // true, in packet order (a calm-down that sends 17 then sets 18 to false
                        // therefore also sounds, like the game).
                        if i == 17 && e.kind == "enderman" && e.meta_bool(18) {
                            stare |= crate::entity_fx::stare_ready(e);
                        }
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
                // The local player is read directly by the frame: only table entities are noted.
                if subset_data && let Some(e) = self.map.get(&id) {
                    self.subsets.note_data(id, e);
                }
                if stare && let Some(e) = self.map.get(&id) {
                    let eye = f64::from(e.size(&self.dims).eye);
                    self.pending_sounds.push(crate::entity_fx::FxSound {
                        name: "minecraft:entity.enderman.stare",
                        at: [e.pos[0], e.pos[1] + eye, e.pos[2]],
                        volume: 2.5,
                        pitch: 1.0,
                        follow: None,
                    });
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
                // Unknown passengers are skipped, like the game (the server sends the packet again
                // when it pairs them): a passenger is then always known while listed.
                let known = |p: &i32| *p == self.local || self.map.contains_key(p);
                let passengers: Vec<i32> = passengers.into_iter().filter(known).collect();
                self.leave_previous_vehicles(vehicle, &passengers);
                // Former passengers detached, new ones attached to the mount.
                if !passengers.is_empty() && !self.riders.contains(&vehicle) {
                    self.riders.push(vehicle);
                }
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

    /// Digging sniffer (state 5 in entity data 18): 0.4 lower, eyes at 0.81; the age lock
    /// (entity data 17) changes nothing. Before: the state was read from 17, so the size was never applied.
    #[test]
    fn digging_sniffer_is_lower() {
        let table = [[1.9f32, 1.75, 1.05, 2.0]];
        let mut e = spawn("sniffer", [0.0; 3], 0);
        let stand = e.size(&table);
        assert!((stand.h - 1.75).abs() < 1e-6, "{}", stand.h);
        e.meta.insert(18, Meta::Variant(5, 0));
        let dig = e.size(&table);
        assert!((dig.h - 1.35).abs() < 1e-6 && (dig.eye - 0.81).abs() < 1e-6, "{} {}", dig.h, dig.eye);
        assert!((dig.w - 1.9).abs() < 1e-6);
        e.meta.insert(18, Meta::Variant(4, 0));
        e.meta.insert(17, Meta::Int(5));
        assert!((e.size(&table).h - 1.75).abs() < 1e-6);
    }

    /// Event 55: both hands of a living entity swap, the rest of the equipment does not move;
    /// a non-living entity ignores it.
    #[test]
    fn hand_swap_event_swaps_both_hands() {
        let st = |item| Some(ItemStack { item, count: 1, added: Vec::new(), removed: Vec::new() });
        for (kind, swaps) in [("player", true), ("zombie", true), ("armor_stand", true), ("item", false)] {
            let mut all = Entities { type_names: vec![format!("minecraft:{kind}")], ..Default::default() };
            let add = rc_proto_v777::entity::AddEntity {
                id: 1,
                uuid: 1,
                kind: 0,
                pos: [0.0; 3],
                velocity: [0.0; 3],
                pitch: 0.0,
                yaw: 0.0,
                head_yaw: 0.0,
                data: 0,
            };
            all.apply(EntityEvent::Add(Box::new(add)));
            let e = all.map.get_mut(&1).expect("added");
            e.equipment[0] = st(10);
            e.equipment[1] = st(20);
            e.equipment[5] = st(30);
            all.apply(EntityEvent::Event { id: 1, event: 55 });
            let eq = &all.map[&1].equipment;
            let (main, off) = if swaps { (20, 10) } else { (10, 20) };
            assert_eq!(eq[0].as_ref().map(|s| s.item), Some(main), "{kind}");
            assert_eq!(eq[1].as_ref().map(|s| s.item), Some(off), "{kind}");
            assert_eq!(eq[5].as_ref().map(|s| s.item), Some(30), "{kind}");
        }
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
        // The player (9) is the local one: a passenger the client knows.
        let mut all = Entities { type_names: vec!["minecraft:oak_boat".into()], local: 9, ..Default::default() };
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

    /// A standing mob whose first passenger is a mob keeps its body rotation when its head
    /// turns; alone, its body follows the head. Before: both followed.
    #[test]
    fn standing_mob_carrying_a_mob_keeps_its_body() {
        let mut all = Entities {
            type_names: vec!["minecraft:spider".into(), "minecraft:skeleton".into()],
            ..Default::default()
        };
        for (id, kind) in [(1, 0), (2, 1), (3, 0)] {
            let add = rc_proto_v777::entity::AddEntity {
                id,
                uuid: id as u128,
                kind,
                pos: [f64::from(id) * 4.0, 64.0, 0.0],
                velocity: [0.0; 3],
                pitch: 0.0,
                yaw: 0.0,
                head_yaw: 0.0,
                data: 0,
            };
            all.apply(EntityEvent::Add(Box::new(add)));
        }
        all.apply(EntityEvent::Passengers { vehicle: 1, passengers: vec![2] });
        for id in [1, 3] {
            let e = all.map.get_mut(&id).expect("spider");
            (e.head, e.body, e.stable_head) = (100.0, 0.0, 0.0);
        }
        all.tick();
        assert_eq!(all.map[&1].body, 0.0, "the jockey turned its body");
        assert!(all.map[&3].body != 0.0, "the lone spider kept its body");
        // The vehicle list and the walk of every entity mark the same riders; a dismount resets.
        let flags = |a: &Entities| {
            let mut v: Vec<(i32, bool)> = a.map.iter().map(|(i, e)| (*i, e.carries_mob)).collect();
            v.sort_unstable();
            v
        };
        let listed = flags(&all);
        all.mark_mob_riders_with(false);
        assert_eq!(flags(&all), listed);
        all.apply(EntityEvent::Passengers { vehicle: 1, passengers: vec![] });
        all.tick();
        assert!(!all.map[&1].carries_mob && all.riders.is_empty(), "dismounted spider still marked");
    }

    /// Micro-bench (ignored, `--ignored --nocapture`): the mob-rider pass of a tick against the
    /// whole tick, 3,000 zombies, 10 with a zombie passenger.
    #[test]
    #[ignore]
    fn bench_mark_mob_riders() {
        let mut all = Entities { type_names: vec!["minecraft:zombie".into()], ..Default::default() };
        for id in 0..3000 {
            let pos = [f64::from(id % 60), 64.0, f64::from(id / 60)];
            let add = rc_proto_v777::entity::AddEntity {
                id,
                uuid: id as u128,
                kind: 0,
                pos,
                velocity: [0.0; 3],
                pitch: 0.0,
                yaw: 0.0,
                head_yaw: 0.0,
                data: 0,
            };
            all.apply(EntityEvent::Add(Box::new(add)));
        }
        for v in 0..10 {
            all.apply(EntityEvent::Passengers { vehicle: v * 2, passengers: vec![v * 2 + 1] });
        }
        for _ in 0..2 {
            let mut us = [0.0; 2];
            for (k, list) in [false, true].into_iter().enumerate() {
                let t0 = std::time::Instant::now();
                for _ in 0..1000 {
                    all.mark_mob_riders_with(list);
                }
                us[k] = t0.elapsed().as_secs_f64() * 1.0e3;
            }
            let t0 = std::time::Instant::now();
            for _ in 0..100 {
                all.tick();
            }
            let tick = t0.elapsed().as_secs_f64() * 1.0e4;
            eprintln!("mob riders: walk {:.2} us, vehicle list {:.3} us per tick; whole tick {tick:.1} us (3,000 entities)", us[0], us[1]);
        }
        assert!(all.map[&0].carries_mob && !all.map[&2999].carries_mob && all.riders.len() == 10);
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

/// Switches converted to `rc_data::opt_switch!`: each one still follows its own variable,
/// with the rule of the reader it replaced (`env_str!(name) != Some("0")`). The environment
/// cannot be modified in the process, so a child copy of this test binary is started with the
/// variables set to each value.
#[cfg(test)]
mod switch_reading_tests {
    use crate::entity_render::EntOpt;

    /// Function that returns the state of one switch.
    type SwitchReader = fn() -> bool;

    /// Every converted switch with its reader (all on by default).
    const SWITCHES: [(&str, SwitchReader); 21] = [
        ("RUSTCRAFT_OPT_NAME_TAG_EARLY_REJECT", crate::app_entity_sets::name_tag_early_reject_on),
        ("RUSTCRAFT_OPT_HOLDER_NAME_ON_STACK", crate::app_entity_sets::holder_name_on_stack_on),
        ("RUSTCRAFT_OPT_BELOW_NAME_KEY_CACHE", crate::app_entity_sets::below_name_key_cache_on),
        ("RUSTCRAFT_OPT_PLAYER_SET_REUSE", crate::app_entity_sets::player_set_reuse_on),
        ("RUSTCRAFT_OPT_ENTITY_SUBSETS", super::entity_subsets_on),
        ("RUSTCRAFT_OPT_RARE_KIND_LISTS", super::rare_kind_lists_on),
        ("RUSTCRAFT_OPT_WET_FLAG_IN_ENTITY", crate::entity_fx::wet_flag_in_entity_on),
        ("RUSTCRAFT_OPT_VARIANTIDX", super::variant_idx_on),
        ("RUSTCRAFT_OPT_NAMECACHE", super::name_cache_on),
        ("RUSTCRAFT_OPT_RIDERLIST", super::rider_list_on),
        ("RUSTCRAFT_OPT_UPLOADRING", crate::render::upload::ring_on),
        ("RUSTCRAFT_OPT_FRAMEUNIF", crate::render::upload::merged_on),
        ("RUSTCRAFT_OPT_UNIFSKIP", crate::render::upload::skip_on),
        ("RUSTCRAFT_OPT_TAGPERSIST", crate::nametag::persist_on),
        ("RUSTCRAFT_OPT_EQUIPFLAG", || EntOpt::from_env().equip_flag),
        ("RUSTCRAFT_OPT_KINDINFO", || EntOpt::from_env().kind_info),
        ("RUSTCRAFT_OPT_SIZEONCE", || EntOpt::from_env().size_once),
        ("RUSTCRAFT_OPT_OBJEARLY", || EntOpt::from_env().obj_early),
        ("RUSTCRAFT_OPT_LIGHTONCE", || EntOpt::from_env().light_once),
        ("RUSTCRAFT_OPT_REGIDX", || EntOpt::from_env().reg_idx),
        ("RUSTCRAFT_OPT_ITEMFLAGS", || EntOpt::from_env().item_flags),
    ];

    /// Run in the child process only: prints the value of every switch.
    #[test]
    #[ignore = "started by switches_follow_their_variable with the variables set"]
    fn print_switch_values() {
        for (name, read) in SWITCHES {
            // Read twice: the cached second read must give the first value.
            let first = read();
            assert_eq!(read(), first, "{name}");
            println!("switch {name}={first}");
        }
    }

    /// Values printed by a child started with `env` (the other switches removed).
    fn child_values(env: &[(&str, &str)]) -> String {
        let mut child = std::process::Command::new(std::env::current_exe().expect("test binary path"));
        child.args(["--exact", "entities::switch_reading_tests::print_switch_values", "--ignored", "--nocapture"]);
        for (name, _) in SWITCHES {
            child.env_remove(name);
        }
        child.envs(env.iter().copied());
        let out = child.output().expect("child test binary");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(out.status.success(), "child failed under {env:?}:\n{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
        stdout
    }

    #[test]
    fn switches_follow_their_variable() {
        // Every switch set to the same value, then each one alone at `0` (catches a site that
        // reads the name of another switch).
        let mut cases: Vec<Vec<(&str, &str)>> = vec![Vec::new()];
        for value in ["0", "1", ""] {
            cases.push(SWITCHES.iter().map(|&(name, _)| (name, value)).collect());
        }
        cases.extend(SWITCHES.iter().map(|&(name, _)| vec![(name, "0")]));
        for env in cases {
            let stdout = child_values(&env);
            for (name, _) in SWITCHES {
                // Old reader: `env_str!(name) != Some("0")`.
                let value = env.iter().find(|(n, _)| *n == name).map(|&(_, v)| v);
                let expected = value != Some("0");
                let line = format!("switch {name}={expected}");
                assert!(stdout.lines().any(|l| l == line), "{name} under {env:?}: expected {expected}\n{stdout}");
            }
        }
    }
}
