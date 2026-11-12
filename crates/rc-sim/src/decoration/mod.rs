//! Decorative entities simulated by the integrated server: paintings, frames (glowing or not),
//! armor stands, cushions, mannequins. Placement by the item, survival of what is hung,
//! breaking and loot, interactions, data sent to the client.
//!
//! The NBT compound kept in `Server::entities` remains the truth (save, `/data`, `/kill`,
//! selectors unchanged); `Decos` only holds the state the game does not save: counter of
//! the survival check and instant of the last hit dealt to an armor stand.

mod cushion;
mod hanging;
mod mannequin;
pub(crate) mod stand;
mod summoned;
#[cfg(test)]
mod switch_equivalence_tests;
#[cfg(test)]
mod tests;

pub(crate) use summoned::summoned;

use hashbrown::HashSet;

use rc_client::{EntityEvent, Event};
use rc_physics::Aabb;
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use crate::blocks::{Dir, P};
use crate::nbtio::{self, int};
use crate::player::Stack;
use crate::server::Server;

/// Kind of decorative entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DKind {
    Painting,
    Frame,
    GlowFrame,
    Stand,
    Cushion,
    Mannequin,
    /// Lead knot on a fence (G4-F07a, never saved).
    Knot,
}

impl DKind {
    pub fn from_name(n: &str) -> Option<DKind> {
        Some(match n {
            "minecraft:painting" => DKind::Painting,
            "minecraft:item_frame" => DKind::Frame,
            "minecraft:glow_item_frame" => DKind::GlowFrame,
            "minecraft:armor_stand" => DKind::Stand,
            "minecraft:cushion" => DKind::Cushion,
            "minecraft:mannequin" => DKind::Mannequin,
            "minecraft:leash_knot" => DKind::Knot,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            DKind::Painting => "minecraft:painting",
            DKind::Frame => "minecraft:item_frame",
            DKind::GlowFrame => "minecraft:glow_item_frame",
            DKind::Stand => "minecraft:armor_stand",
            DKind::Cushion => "minecraft:cushion",
            DKind::Mannequin => "minecraft:mannequin",
            DKind::Knot => "minecraft:leash_knot",
        }
    }

    /// Hung on a wall or ceiling (painting, frames).
    pub fn hanging(self) -> bool {
        matches!(self, DKind::Painting | DKind::Frame | DKind::GlowFrame)
    }

    pub fn frame(self) -> bool {
        matches!(self, DKind::Frame | DKind::GlowFrame)
    }

    /// Attached to a block: survival checked every 100 ticks.
    fn attached(self) -> bool {
        self.hanging() || matches!(self, DKind::Cushion | DKind::Knot)
    }
}

/// False for an entity the game never announces to clients (marker: server-side only).
pub(crate) fn client_visible(e: &Nbt) -> bool {
    e.get("id").and_then(Nbt::as_str) != Some("minecraft:marker")
}

/// Kind of an entity compound.
pub(crate) fn kind_of(e: &Nbt) -> Option<DKind> {
    e.get("id").and_then(Nbt::as_str).and_then(DKind::from_name)
}

/// Position of a decoration whose moves the network tracking sends (armor stand, mannequin).
pub(crate) fn tracked_position(e: &Nbt) -> Option<[f64; 3]> {
    if !matches!(kind_of(e), Some(DKind::Stand | DKind::Mannequin)) {
        return None;
    }
    nbtio::f64s::<3>(e.get("Pos"))
}

/// Push scans reach each compound from its registry entry (`RUSTCRAFT_OPT_DECOSCAN=0`: by
/// identifier, the former lookup).
fn deco_scan_indexed() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_DECOSCAN") != Some("0"))
}

/// Optimisation switches of the decoration code (and of the blast pass over the still
/// compounds), read once at startup; a test flips them on its own server to compare both paths.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DecoSwitches {
    /// Callers that know a decoration's registry index use it (edit, push, survival) instead of
    /// walking the registry for its identifier (`RUSTCRAFT_OPT_DECO_SINGLE_LOOKUP=0`).
    pub by_index: bool,
    /// The compound's remembered place in its chunk's entity list is tried before scanning the
    /// list (`RUSTCRAFT_OPT_DECO_SLOT_HINT=0`).
    pub slot_hint: bool,
    /// An explosion reads each compound in place and copies only those it breaks
    /// (`RUSTCRAFT_OPT_DECO_EXPLOSION_BORROW=0`: a deep copy of every decoration per blast).
    pub explosion_borrow: bool,
    /// The blast pass over the still compounds tests the distance before the entity type
    /// (`RUSTCRAFT_OPT_STILL_PUSH_DISTANCE_FIRST=0`: the type first, the former order).
    pub still_distance_first: bool,
}

impl Default for DecoSwitches {
    fn default() -> DecoSwitches {
        DecoSwitches {
            by_index: rc_data::opt_switch!("RUSTCRAFT_OPT_DECO_SINGLE_LOOKUP"),
            slot_hint: rc_data::opt_switch!("RUSTCRAFT_OPT_DECO_SLOT_HINT"),
            explosion_borrow: rc_data::opt_switch!("RUSTCRAFT_OPT_DECO_EXPLOSION_BORROW"),
            still_distance_first: rc_data::opt_switch!("RUSTCRAFT_OPT_STILL_PUSH_DISTANCE_FIRST"),
        }
    }
}

/// Index of entity `id` in a chunk's entity list, trying `slot` (where it was last found) first.
fn index_in(list: &[(i32, Nbt)], id: i32, slot: Option<usize>) -> Option<usize> {
    if let Some(k) = slot
        && list.get(k).is_some_and(|(x, _)| *x == id)
    {
        return Some(k);
    }
    list.iter().position(|(x, _)| *x == id)
}

/// Result of an item use on a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Use {
    Fail,
    Consume,
    Success,
}

#[derive(Debug, Clone)]
struct Live {
    id: i32,
    kind: DKind,
    /// Chunk where the compound was last seen (fast lookup).
    chunk: (i32, i32),
    /// Its index in that chunk's entity list when last seen: a hint only, checked before use (a
    /// removal or a move out of the list shifts it). Saves scanning a crowded chunk's list.
    slot: usize,
    /// Ticks since the last survival check (not saved, 0 at creation).
    check: i32,
    /// Armor stand: instant of the last hit that made it wobble.
    last_hit: i64,
    removed: bool,
    /// Network tracking of a decoration that moves (armor stand, mannequin).
    track: crate::mobs::track::Tracker,
    /// Mannequin: hurt cooldown (ticks) and the damage of the last hit (not saved, like the game).
    hurt_cooldown: i32,
    last_hurt: f32,
    /// Piston pushes of the current tick (armor stand, mannequin: each entity its own cap).
    piston: rc_physics::piston::PistonDeltas,
    /// Rank in the entity walk (`walk::next_rank`), stamped when it enters the list.
    rank: u64,
    /// Past its first tick in this session: before it, a living one offers no waypoint connection.
    ticked: bool,
}

impl Live {
    fn new(id: i32, kind: DKind, chunk: (i32, i32), slot: usize) -> Live {
        Live {
            id,
            kind,
            chunk,
            slot,
            check: 0,
            last_hit: 0,
            removed: false,
            track: Default::default(),
            hurt_cooldown: 0,
            last_hurt: 0.0,
            piston: Default::default(),
            rank: crate::walk::next_rank(),
            ticked: false,
        }
    }
}

/// A living decoration (armor stand, mannequin) as the locator bar rules see it: like any living
/// entity of the game, it transmits a waypoint once its transmit range is above zero (a command,
/// its saved data or a worn stack of its own gives it one).
pub(crate) struct DecoTransmitter<'a> {
    pub id: i32,
    /// Past its first tick: a brand-new entity offers no connection yet.
    pub ticked: bool,
    pub transmit_range: f64,
    pub compound: &'a Nbt,
}

/// A living decoration (armor stand, mannequin) as an area effect sees it (`deco_living`).
pub(crate) struct LivingDeco {
    pub id: i32,
    pub uuid: u128,
    pub pos: [f64; 3],
    pub bbox: Aabb,
    pub marker: bool,
}

/// An explosion as the decoration pass sees it.
struct DecoBlast {
    center: [f64; 3],
    /// Twice the radius: the reach, and the damage scale of a mannequin.
    diameter: f64,
    /// A mob is the author and the `mob_griefing` rule is off: nothing is broken.
    refused_by_rule: bool,
    /// The direct source is in water: nothing hung or laid is touched.
    wet: bool,
    /// The explosion touches block-like entities.
    blocklike: bool,
}

/// What an explosion does to one decoration.
enum DecoBlastEffect {
    /// Mannequin hurt by `damage`, then pushed by `knockback`.
    HurtMannequin { damage: f32, knockback: [f64; 3] },
    /// Armor stand or mannequin left standing (refused, marker), pushed.
    Push([f64; 3]),
    /// Armor stand smashed, its equipment dropped (copy of its compound).
    BreakStand(Nbt),
    /// Something attached killed with its loot, attributed to the author when `credited`.
    KillAttached { compound: Nbt, credited: bool },
}

/// Where a decoration stands in a blast (explosion or wind burst): the part both read before
/// deciding anything. The exposure, costly, is reckoned apart ([`Server::deco_blast_exposure`]).
struct BlastReach {
    pos: [f64; 3],
    /// Distance to the center over the blast's diameter, at most 1.
    dist: f64,
    bbox: Aabb,
}

/// Fields of an entry read by the tick (without copying the network tracking).
struct Brief {
    id: i32,
    kind: DKind,
    check: i32,
    removed: bool,
}

/// Decorations of the open world (exchanged with the dimension).
#[derive(Default)]
pub(crate) struct Decos {
    live: Vec<Live>,
    /// Entity chunks already read (single registration).
    pub(crate) adopted: HashSet<(i32, i32)>,
    /// Cushion on which the player is sitting.
    pub seat: Option<i32>,
    /// Explosion about to go off: its non-mob direct source (TNT, projectile) is in
    /// water. Read and reset to false by the explosion.
    pub blast_wet: bool,
    pub(crate) switches: DecoSwitches,
}

// ---------- Directions and boxes ----------

/// Direction read as a 3D value of the game (down, up, north, south, west, east), modulo 6.
pub(crate) fn dir_3d(v: i64) -> Dir {
    Dir::from_index(v.unsigned_abs() as usize % 6)
}

/// Direction read as a 2D value of the game (south, west, north, east), modulo 4.
pub(crate) fn dir_2d(v: i64) -> Dir {
    [Dir::South, Dir::West, Dir::North, Dir::East][v.unsigned_abs() as usize % 4]
}

/// 2D value of a horizontal direction (−1 for a vertical axis).
pub(crate) fn value_2d(d: Dir) -> i32 {
    match d {
        Dir::South => 0,
        Dir::West => 1,
        Dir::North => 2,
        Dir::East => 3,
        _ => -1,
    }
}

fn step(d: Dir) -> [f64; 3] {
    d.step().map(f64::from)
}

/// Centered box.
pub(crate) fn of_size(c: [f64; 3], s: [f64; 3]) -> Aabb {
    Aabb {
        min: [c[0] - s[0] / 2.0, c[1] - s[1] / 2.0, c[2] - s[2] / 2.0],
        max: [c[0] + s[0] / 2.0, c[1] + s[1] / 2.0, c[2] + s[2] / 2.0],
    }
}

/// Block center shifted towards the hanging wall (the center of a block).
fn against_wall(pos: P, d: Dir) -> [f64; 3] {
    let s = step(d);
    std::array::from_fn(|i| f64::from(pos[i]) + 0.5 + s[i] * -0.46875)
}

/// Box of a `w × h` painting hung at `pos`, face `d`.
pub(crate) fn painting_box(pos: P, d: Dir, w: i32, h: i32) -> Aabb {
    let off = |n: i32| if n % 2 == 0 { 0.5 } else { 0.0 };
    let wall = against_wall(pos, d);
    let left = step(d.counter_clockwise());
    let c = [wall[0] + left[0] * off(w), wall[1] + left[1] * off(w) + off(h), wall[2] + left[2] * off(w)];
    let x_axis = d.axis() == 0;
    let z_axis = d.axis() == 2;
    of_size(
        c,
        [if x_axis { 0.0625 } else { f64::from(w) }, f64::from(h), if z_axis { 0.0625 } else { f64::from(w) }],
    )
}

/// Box of a frame hung at `pos`, face `d`; a map enlarges it by a quarter.
pub(crate) fn frame_box(pos: P, d: Dir, map: bool) -> Aabb {
    let side = if map { 1.0 } else { 0.75 };
    let a = d.axis();
    of_size(
        against_wall(pos, d),
        [
            if a == 0 { 0.0625 } else { side },
            if a == 1 { 0.0625 } else { side },
            if a == 2 { 0.0625 } else { side },
        ],
    )
}

fn dbl(v: [f64; 3]) -> Nbt {
    Nbt::List(v.iter().map(|&x| Nbt::Double(x)).collect())
}

/// Common fields of a new entity in the game's order (writing without identifier):
/// identifier, position, zero motion, rotation, fire 0, air 300, UUID.
pub(crate) fn base_fields(k: DKind, pos: [f64; 3], yaw: f32, pitch: f32, uuid: u128) -> Vec<(String, Nbt)> {
    vec![
        ("id".into(), Nbt::String(k.name().into())),
        ("Pos".into(), dbl(pos)),
        ("Motion".into(), dbl([0.0; 3])),
        ("Rotation".into(), Nbt::List(vec![Nbt::Float(yaw), Nbt::Float(pitch)])),
        ("fall_distance".into(), Nbt::Double(0.0)),
        ("Fire".into(), Nbt::Short(0)),
        ("Air".into(), Nbt::Short(300)),
        ("OnGround".into(), Nbt::Byte(0)),
        ("Invulnerable".into(), Nbt::Byte(0)),
        ("PortalCooldown".into(), Nbt::Int(0)),
        ("UUID".into(), nbtio::uuid_nbt(uuid)),
    ]
}

/// Field inserted right after `after` (field order of the game), replaced if it exists.
fn put_after(n: &mut Nbt, after: &str, key: &str, v: Nbt) {
    let Nbt::Compound(f) = n else { return };
    if let Some(slot) = f.iter_mut().find(|(k, _)| k == key) {
        slot.1 = v;
        return;
    }
    let at = f.iter().position(|(k, _)| k == after).map_or(f.len(), |i| i + 1);
    f.insert(at, (key.to_owned(), v));
}

fn floor_pos(p: [f64; 3]) -> P {
    p.map(|v| v.floor() as i32)
}

/// Chunk column holding position `p` (kept compounds are filed by it).
pub(crate) fn chunk_of(p: [f64; 3]) -> (i32, i32) {
    ((p[0].floor() as i32) >> 4, (p[2].floor() as i32) >> 4)
}

/// Stack with the entity's custom name (custom name component).
fn named(id: &str, e: &Nbt) -> Stack {
    let mut s = Stack { id: id.to_owned(), count: 1, components: None };
    if let Some(n) = e.get("CustomName") {
        crate::items::set_component(&mut s, "minecraft:custom_name", n.clone());
    }
    s
}

fn flag(e: &Nbt, k: &str) -> bool {
    int(e, k).is_some_and(|v| v != 0)
}

/// Dye colors in the order of their identifiers.
pub(crate) const DYES: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

impl Server {
    // ---------- Registry ----------

    /// Decorations read with entity chunks: registered (once per chunk).
    pub(crate) fn deco_adopt(&mut self, loaded: &[(i32, i32)]) {
        for p in loaded {
            if !self.deco.adopted.insert(*p) {
                continue;
            }
            let Some(list) = self.entities.get(p) else { continue };
            for (slot, (id, e)) in list.iter().enumerate() {
                if let Some(kind) = kind_of(e) {
                    self.deco.live.push(Live::new(*id, kind, *p, slot));
                }
            }
        }
    }

    /// Entity summoned outside placement (`/summon`): registered if decorative.
    pub(crate) fn deco_register(&mut self, id: i32, e: &Nbt) {
        if let Some(kind) = kind_of(e) {
            let chunk = chunk_of(nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default());
            // Its place in the chunk's list is not known yet: the first lookup records it.
            self.deco.live.push(Live::new(id, kind, chunk, 0));
        }
    }

    /// Position of a decoration of the registry (searched first in its chunk); the place where
    /// its compound was found is remembered for the lookups that follow in the same tick.
    pub(crate) fn live_pos(&mut self, i: usize) -> Option<[f64; 3]> {
        let (chunk, slot, compound) = self.find_live(i)?;
        let pos = nbtio::f64s::<3>(compound.get("Pos"));
        let l = &mut self.deco.live[i];
        (l.chunk, l.slot) = (chunk, slot);
        Some(pos.unwrap_or_default())
    }

    /// Chunk, list index and compound of registry entry `i`: its known chunk first (at its
    /// remembered index when that switch is on), otherwise the whole world.
    fn find_live(&self, i: usize) -> Option<((i32, i32), usize, &Nbt)> {
        let l = &self.deco.live[i];
        let slot = self.deco.switches.slot_hint.then_some(l.slot);
        self.find_kept(l.id, Some(l.chunk), slot)
    }

    /// Chunk, list index and compound of kept entity `id`, searched first in chunk `hint`.
    fn find_kept(&self, id: i32, hint: Option<(i32, i32)>, slot: Option<usize>) -> Option<((i32, i32), usize, &Nbt)> {
        if let Some(c) = hint
            && let Some(list) = self.entities.get(&c)
            && let Some(k) = index_in(list, id, slot)
        {
            return Some((c, k, &list[k].1));
        }
        self.entities.iter().find_map(|(c, list)| {
            let k = list.iter().position(|(x, _)| *x == id)?;
            Some((*c, k, &list[k].1))
        })
    }

    fn deco_live(&mut self, id: i32) -> Option<&mut Live> {
        self.deco.live.iter_mut().find(|l| l.id == id && !l.removed)
    }

    /// Decoration `id` has ticked since it entered the world (summoned, placed or loaded): the
    /// game's "first tick" flag, cleared at the end of an entity's first tick and never saved.
    pub(crate) fn deco_past_first_tick(&self, id: i32) -> bool {
        self.deco.live.iter().any(|l| l.id == id && l.ticked)
    }

    /// Compound of a present decoration.
    pub(crate) fn deco_nbt(&self, id: i32) -> Option<&Nbt> {
        let hint = self.deco.live.iter().find(|l| l.id == id).map(|l| l.chunk);
        self.deco_nbt_near(id, hint)
    }

    /// Compound of decoration `i` of the registry (known chunk first: called every tick).
    fn live_nbt(&self, i: usize) -> Option<&Nbt> {
        self.find_live(i).map(|(_, _, compound)| compound)
    }

    /// Searches first in chunk `hint`, otherwise in the whole world.
    fn deco_nbt_near(&self, id: i32, hint: Option<(i32, i32)>) -> Option<&Nbt> {
        self.find_kept(id, hint, None).map(|(_, _, compound)| compound)
    }

    /// Modifies the compound of a decoration; its chunk will be rewritten, and a move into
    /// another chunk files it there (armor stand that falls or is pushed across a border).
    pub(crate) fn deco_edit(&mut self, id: i32, f: impl FnOnce(&mut Nbt)) {
        let hint = self.deco.live.iter().find(|l| l.id == id).map(|l| l.chunk);
        let Some((chunk, slot, _)) = self.find_kept(id, hint, None) else { return };
        self.edit_kept_at(id, chunk, slot, f);
    }

    /// [`Server::deco_edit`] of registry entry `i`, found from the entry itself: no walk of the
    /// registry for its identifier, which made every armor stand falling at once quadratic
    /// (`RUSTCRAFT_OPT_DECO_SINGLE_LOOKUP=0`: that walk, the former path).
    fn deco_edit_at(&mut self, i: usize, f: impl FnOnce(&mut Nbt)) {
        let id = self.deco.live[i].id;
        if !self.deco.switches.by_index {
            return self.deco_edit(id, f);
        }
        let Some((chunk, slot, _)) = self.find_live(i) else { return };
        (self.deco.live[i].chunk, self.deco.live[i].slot) = (chunk, slot);
        self.edit_kept_at(id, chunk, slot, f);
    }

    /// Edit of a decoration's compound, entry `slot` of chunk `chunk`'s list (just found).
    fn edit_kept_at(&mut self, id: i32, chunk: (i32, i32), slot: usize, f: impl FnOnce(&mut Nbt)) {
        let Some((_, n)) = self.entities.get_mut(&chunk).and_then(|list| list.get_mut(slot)) else { return };
        f(n);
        let (filed_under, now_in) = (chunk, chunk_of(nbtio::f64s::<3>(n.get("Pos")).unwrap_or_default()));
        self.life.drop_chunks.insert(filed_under);
        // Cheap check first: the refile searches the whole map, rare (a border crossed).
        if now_in != filed_under {
            self.refile_kept_entity(id);
        }
    }

    /// Announces decoration `id` (`e`: its compound) and, for one whose tracking began (armor
    /// stand, mannequin), codes its next relative move from the announced position. The game
    /// announces at the tracking's last sent position, then sends the move to the current one in
    /// the same tick, even frozen or outside the entity simulation range; announcing straight at
    /// the current position leaves the clients in that same state.
    pub(crate) fn deco_announce_tracked(&mut self, id: i32, e: &Nbt) {
        self.deco_announce(id, e);
        if let Some(pos) = tracked_position(e) {
            self.deco_announced_at(id, pos);
        }
    }

    /// The clients were just given tracked decoration `id` at `pos`.
    pub(crate) fn deco_announced_at(&mut self, id: i32, pos: [f64; 3]) {
        if let Some(l) = self.deco.live.iter_mut().find(|l| l.id == id && !l.removed) {
            l.track.announced_at(pos);
        }
    }

    /// The compound of decoration `id` is now filed under `chunk` (fast lookup kept right).
    pub(crate) fn deco_moved_to_chunk(&mut self, id: i32, chunk: (i32, i32)) {
        for l in self.deco.live.iter_mut().filter(|l| l.id == id) {
            l.chunk = chunk;
        }
    }

    /// Random version 4 UUID, IETF variant.
    pub(crate) fn deco_uuid(&mut self) -> u128 {
        let r = &mut self.life.rng;
        let u = (r.next_long() as u128) << 64 | r.next_long() as u64 as u128;
        let u = (u & !(0xF000u128 << 64)) | (0x4000u128 << 64);
        (u & !(0xC000_0000_0000_0000u128)) | 0x8000_0000_0000_0000u128
    }

    /// Adds a placed decoration: identifier, announcement to the client, registry.
    pub(crate) fn deco_spawn(&mut self, n: Nbt) -> i32 {
        self.next_entity += 1;
        let id = self.next_entity;
        let pos = nbtio::f64s::<3>(n.get("Pos")).unwrap_or_default();
        let c = chunk_of(pos);
        self.deco_announce(id, &n);
        self.deco_register(id, &n);
        self.entities.entry(c).or_default().push((id, n));
        self.life.drop_chunks.insert(c);
        id
    }

    /// Removes a decoration (`discard`/`kill`): compound, registry, client.
    pub(crate) fn deco_remove(&mut self, id: i32) {
        let mut at = None;
        let mut visible = true;
        for (c, l) in self.entities.iter_mut() {
            if let Some(i) = l.iter().position(|(x, _)| *x == id) {
                at = nbtio::f64s::<3>(l[i].1.get("Pos"));
                visible = client_visible(&l[i].1);
                l.remove(i);
                self.life.drop_chunks.insert(*c);
                break;
            }
        }
        self.cushion_ejected(id, at);
        for l in self.deco.live.iter_mut().filter(|l| l.id == id) {
            l.removed = true;
        }
        if visible {
            let _ = self.send(Event::Entity(EntityEvent::Remove(vec![id])));
        }
    }

    // ---------- Client ----------

    /// Appearance on the client of a decoration (or of any frozen entity): painting and frames
    /// announced at the corner of the hanging block with their 3D direction, like the game's server.
    pub(crate) fn deco_announce(&self, id: i32, e: &Nbt) {
        if !client_visible(e) {
            return;
        }
        let types = self.data.registry("minecraft:entity_type");
        let Some(kind) =
            e.get("id").and_then(Nbt::as_str).and_then(|n| types?.entries.iter().position(|t| t == n))
        else {
            return;
        };
        let mut pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        let [yaw, pitch] = nbtio::f32s::<2>(e.get("Rotation")).unwrap_or_default();
        let mut data = int(e, "Facing").unwrap_or(0) as i32;
        if let Some(k) = kind_of(e)
            && k.hanging()
        {
            let (p, d) = self.hang_place(k, e);
            pos = p.map(f64::from);
            data = d as i32;
        } else if kind_of(e) == Some(DKind::Knot) {
            // Knot: announced at the corner of its block, null data.
            pos = block_pos_of(e).map(f64::from);
            data = 0;
        }
        let add = rc_proto_v777::entity::AddEntity {
            id,
            uuid: nbtio::uuid(e.get("UUID")).unwrap_or(0),
            kind: kind as i32,
            pos,
            velocity: nbtio::motion(e.get("Motion")).unwrap_or_default(),
            pitch,
            yaw,
            head_yaw: yaw,
            data,
        };
        let _ = self.send(Event::Entity(EntityEvent::Add(Box::new(add))));
        self.send_entity_meta(id, e);
        // A living one: its synchronised attributes held by the game (scale, armor…).
        if let Some(DKind::Stand | DKind::Mannequin) = kind_of(e)
            && let Some(ty) = e.get("id").and_then(Nbt::as_str)
        {
            let attributes = self.nbt_synced_attributes(e, ty, None);
            if !attributes.is_empty() {
                let _ = self.send(Event::Attributes { entity: id, attributes });
            }
        }
    }

    /// Data specific to decorations, added to those of `send_entity_meta`.
    pub(crate) fn deco_meta(&self, e: &Nbt, values: &mut Vec<(u8, Meta)>) {
        let Some(k) = kind_of(e) else { return };
        match k {
            DKind::Painting | DKind::Frame | DKind::GlowFrame => {
                let (_, d) = self.hang_place(k, e);
                if d != Dir::South {
                    values.push((8, Meta::Direction(d as i32)));
                }
                if k == DKind::Painting {
                    // Default value of the game: first variant of the registry.
                    if let Some(v) = self.painting_variant_index(e).filter(|&v| v != 0) {
                        values.push((9, Meta::Variant(v as i32, 34)));
                    }
                } else {
                    if let Some(s) = e.get("Item").and_then(Stack::from_nbt) {
                        values.push((9, Meta::Item(self.net(Some(&s)))));
                    }
                    let r = int(e, "ItemRotation").unwrap_or(0) as i32 % 8;
                    if r != 0 {
                        values.push((10, Meta::Int(r)));
                    }
                }
            }
            DKind::Stand => stand::meta(e, values),
            DKind::Cushion => {
                let c = cushion::color(e);
                if c != 0 {
                    values.push((8, Meta::Variant(c as i32, 43)));
                }
            }
            DKind::Mannequin => mannequin_meta(e, values),
            DKind::Knot => {}
        }
    }

    /// Network data of a changed decoration.
    fn deco_data(&self, id: i32, values: Vec<(u8, Meta)>) {
        if !values.is_empty() {
            let _ = self.send(Event::Entity(EntityEvent::Data { id, values }));
        }
    }

    // ---------- Sounds, loot ----------

    /// Sound emitted by the entity: silent if `Silent`.
    fn deco_sound(&mut self, e: &Nbt, name: &str, category: i32, volume: f32, pitch: f32) {
        if flag(e, "Silent") {
            return;
        }
        let pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        self.world_sound(name, category, pos, volume, pitch);
    }

    /// Item dropped by a hung entity: shifted by 0.15 towards the face.
    fn deco_drop_at(&mut self, e: &Nbt, d: Option<Dir>, s: Stack) {
        let mut pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        if let Some(d) = d {
            let st = step(d);
            pos[0] += st[0] * f64::from(0.15f32);
            pos[2] += st[2] * f64::from(0.15f32);
        }
        self.spawn_item_random(pos, s, 10);
    }

    /// Block shards (`block` particle).
    pub(crate) fn block_particles(&self, state: u32, pos: [f64; 3], delta: [f32; 3], speed: f32, count: i32) {
        let Some(kind) = self
            .data
            .registry("minecraft:particle_type")
            .and_then(|r| r.entries.iter().position(|e| e == "minecraft:block"))
        else {
            return;
        };
        let p = rc_proto_v777::entity::LevelParticles {
            particle: rc_proto_v777::entity::Particle {
                kind: kind as i32,
                color: 0,
                state: state as i32,
                item: -1,
                scale: 1.0,
                color2: 0,
                vib: None,
                trail: None,
            },
            force: false,
            always_visible: false,
            pos,
            delta,
            speed: [speed; 3],
            count,
            mode: 0,
        };
        let _ = self.send(Event::Particles(Box::new(p)));
    }

    // ---------- Tick ----------

    /// The decorations as their own walk (tests): under the world, survival every 100 ticks.
    #[cfg(test)]
    pub(crate) fn deco_tick(&mut self) {
        self.cushion_seat_tick(); // seated player: it ticks even when the world is frozen
        if self.cmd.ticking.runs() {
            self.walk_alone(crate::walk::bit(crate::walk::Fam::Deco), |_| {}, Self::deco_post);
        }
    }

    pub(crate) fn deco_live_len(&self) -> usize {
        self.deco.live.len()
    }

    pub(crate) fn deco_rank(&self, i: usize) -> Option<u64> {
        self.deco.live.get(i).map(|l| l.rank)
    }

    pub(crate) fn deco_rank_mut(&mut self, i: usize) -> Option<&mut u64> {
        self.deco.live.get_mut(i).map(|l| &mut l.rank)
    }

    pub(crate) fn deco_resort(&mut self) {
        crate::walk::sort_by_rank(&mut self.deco.live, |l| l.rank);
    }

    /// A decoration at `pos` ticks: "entity ticking" chunk, simulated and its 8 neighbours
    /// loaded (otherwise a support located in an absent neighbour would be read as void).
    pub(crate) fn deco_ticks_at(&self, pos: [f64; 3]) -> bool {
        let c = chunk_of(pos);
        let loaded =
            (-1..=1).all(|dx| (-1..=1).all(|dz| matches!(self.chunks.get(&(c.0 + dx, c.1 + dz)), Some(Some(_)))));
        self.entities_tick_in(c) && loaded
    }

    /// Decoration of slot `i`, at its rank in the entity walk: under the world, survival every
    /// 100 ticks, the fall of an armor stand or a mannequin.
    pub(crate) fn deco_visit(&mut self, i: usize) {
        let Some(l) = self.deco.live.get(i) else { return };
        let l = Brief { id: l.id, kind: l.kind, check: l.check, removed: l.removed };
        if l.removed {
            return;
        }
        let Some(pos) = self.live_pos(i) else {
            self.deco.live[i].removed = true;
            return;
        };
        let mut rank = self.deco.live[i].rank;
        let meet = self.meet(&mut rank, self.deco_ticks_at(pos), crate::walk::Fam::Deco);
        self.deco.live[i].rank = rank;
        if meet != crate::walk::Meet::Tick {
            return;
        }
        self.deco.live[i].ticked = true;
        if pos[1] < f64::from(self.dim.height.min_y - 64) {
            self.deco_remove(l.id);
            return;
        }
        if !l.kind.attached() {
            self.living_tick(i, pos); // armor stand, mannequin: fall
            return;
        }
        self.deco.live[i].check = l.check + 1;
        if l.check < 100 {
            return;
        }
        self.deco.live[i].check = 0;
        if l.kind == DKind::Cushion {
            self.cushion_fire_check(l.id, true);
            if self.deco.live[i].removed {
                return;
            }
        }
        if !self.deco_survives_at(i) {
            let e = self.live_nbt(i).cloned();
            self.deco_remove(l.id);
            if let Some(e) = e {
                self.deco_drop_item(&e, None);
            }
        }
    }

    /// Removed decorations, after the entity walk.
    pub(crate) fn deco_post(&mut self) {
        if self.deco.live.iter().any(|l| l.removed) {
            self.deco.live.retain(|l| !l.removed);
        }
    }

    /// `survives` of an attached entity.
    pub(crate) fn deco_survives(&self, id: i32) -> bool {
        self.deco_nbt(id).is_some_and(|e| self.attached_survives(id, e))
    }

    /// [`Server::deco_survives`] of registry entry `i` (found from the entry, not by identifier).
    fn deco_survives_at(&self, i: usize) -> bool {
        let id = self.deco.live[i].id;
        if !self.deco.switches.by_index {
            return self.deco_survives(id);
        }
        self.live_nbt(i).is_some_and(|e| self.attached_survives(id, e))
    }

    fn attached_survives(&self, id: i32, e: &Nbt) -> bool {
        match kind_of(e) {
            Some(k) if k.hanging() => self.hang_survives(id, k, e),
            Some(DKind::Cushion) => self.cushion_survives(e),
            Some(DKind::Knot) => {
                let p = block_pos_of(e);
                self.btag_id(p, crate::tag_bits::TagId::Fences)
            }
            _ => true,
        }
    }

    /// Loot of a removed attached entity; `creative`: broken by a player in
    /// creative (nothing drops).
    fn deco_drop_item(&mut self, e: &Nbt, by: Option<crate::mobs::mob::Ent>) {
        let creative = by == Some(crate::mobs::mob::Ent::Player) && self.game_mode == 1;
        match kind_of(e) {
            Some(DKind::Painting) => {
                if self.rule("minecraft:entity_drops", true) {
                    self.deco_sound(e, "minecraft:entity.painting.break", 6, 1.0, 1.0);
                    if !creative {
                        let d = self.hang_place(DKind::Painting, e).1;
                        self.deco_drop_at(e, Some(d), named("minecraft:painting", e));
                    }
                }
            }
            Some(k) if k.frame() => self.frame_broken(k, e, by),
            Some(DKind::Cushion) => self.cushion_dropped(e, by),
            // Knot: no item, only the sound of a detached lead.
            Some(DKind::Knot) => self.knot_dropped(e),
            _ => {}
        }
    }

    // ---------- Explosions ----------

    /// Blast of an explosion of type `ty` on the decorations: hung items and cushions broken
    /// (item dropped), armor stands smashed (equipment dropped), mannequins hurt; a stand or a
    /// mannequin left standing is then pushed (reduced by its explosion knockback resistance);
    /// `by`: author (entity behind the damage source); `wet`: direct source in water, nothing
    /// hung is then touched; `blocklike`: the explosion touches block-like entities.
    pub(crate) fn deco_explosion(
        &mut self,
        center: [f64; 3],
        radius: f32,
        ty: &str,
        by: Option<crate::mobs::mob::Ent>,
        wet: bool,
        blocklike: bool,
    ) {
        if radius < 1.0e-5 || self.deco.live.is_empty() {
            return;
        }
        if self.deco.switches.explosion_borrow {
            self.deco_explosion_borrowed(center, radius, ty, by, wet, blocklike);
        } else {
            self.deco_explosion_cloned(center, radius, ty, by, wet, blocklike);
        }
    }

    /// [`Server::deco_explosion`], former path: a deep copy of every decoration's compound
    /// (equipment, framed items) per blast, far ones included (`RUSTCRAFT_OPT_DECO_EXPLOSION_BORROW=0`).
    fn deco_explosion_cloned(
        &mut self,
        center: [f64; 3],
        radius: f32,
        ty: &str,
        by: Option<crate::mobs::mob::Ent>,
        wet: bool,
        blocklike: bool,
    ) {
        let double = f64::from(radius * 2.0);
        let by_mob = matches!(by, Some(crate::mobs::mob::Ent::Mob(_)));
        let griefing = self.rule("minecraft:mob_griefing", true);
        let ids: Vec<(i32, DKind, (i32, i32))> =
            self.deco.live.iter().filter(|l| !l.removed).map(|l| (l.id, l.kind, l.chunk)).collect();
        for (id, k, chunk) in ids {
            let Some(e) = self.deco_nbt_near(id, Some(chunk)).cloned() else { continue };
            let ignore = match k {
                DKind::Stand => !blocklike || flag(&e, "Invisible"),
                DKind::Mannequin => false,
                _ => wet || !blocklike,
            };
            if ignore {
                continue;
            }
            let Some(b) = self.deco_box(&e) else { continue };
            let pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
            let d = [pos[0] - center[0], pos[1] - center[1], pos[2] - center[2]];
            let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() / double;
            if dist > 1.0 {
                continue;
            }
            let eye = self.deco_eye(k, &e);
            let o = [pos[0] - center[0], pos[1] + eye - center[1], pos[2] - center[2]];
            let olen = (o[0] * o[0] + o[1] * o[1] + o[2] * o[2]).sqrt();
            let exposure = f64::from(self.seen_percent(center, &b));
            // The hit: refused for a mob without the `mob_griefing` rule, or invulnerable.
            let refused = (by_mob && !griefing) || flag(&e, "Invulnerable");
            if matches!(k, DKind::Stand | DKind::Mannequin) {
                if k == DKind::Mannequin {
                    let pow = (1.0 - dist) * exposure;
                    let dmg = ((pow * pow + pow) / 2.0 * 7.0 * double + 1.0) as f32;
                    self.mannequin_hurt(id, ty, dmg, by, Some(center));
                } else if !refused && !flag(&e, "Marker") {
                    let at = pos.map(|v| v.floor() as i32);
                    self.stand_broken(&e, at);
                    self.deco_remove(id);
                    self.game_event_at("minecraft:entity_die", pos, None);
                    continue;
                }
                // Still there (refused, marker, mannequin): pushed like any living entity.
                let res = self.nbt_attribute(&e, k.name(), "minecraft:explosion_knockback_resistance").unwrap_or(0.0);
                let kp = (1.0 - dist) * exposure * (1.0 - res);
                self.deco_push(id, crate::mobs::mth::normalize(o).map(|v| v * kp));
                continue;
            }
            if !refused {
                self.attached_kill_by(id, &e, by); // loot attributed to the author (creative: nothing)
                continue;
            }
            // Non-zero push: the hung item is knocked off (fixed frame excepted).
            let pushed = olen >= f64::from(1.0e-5f32) && (1.0 - dist) * exposure != 0.0;
            if pushed && !(k.frame() && flag(&e, "Fixed")) {
                self.attached_kill_by(id, &e, None);
            }
        }
    }

    /// [`Server::deco_explosion`] reading each compound in place: a decoration out of reach
    /// costs a few field reads, and only one the blast breaks has its compound copied (the
    /// breaking calls change the server while they read it). Same decisions, same order.
    fn deco_explosion_borrowed(
        &mut self,
        center: [f64; 3],
        radius: f32,
        ty: &str,
        by: Option<crate::mobs::mob::Ent>,
        wet: bool,
        blocklike: bool,
    ) {
        let blast = DecoBlast {
            center,
            diameter: f64::from(radius * 2.0),
            refused_by_rule: matches!(by, Some(crate::mobs::mob::Ent::Mob(_))) && !self.rule("minecraft:mob_griefing", true),
            wet,
            blocklike,
        };
        // Registry length fixed at the start, like the former snapshot of identifiers: nothing a
        // blast adds is visited, and a removed entry keeps its index (`removed` set, not dropped).
        for i in 0..self.deco.live.len() {
            let Some(effect) = self.deco_blast_effect(i, &blast) else { continue };
            let id = self.deco.live[i].id;
            match effect {
                DecoBlastEffect::HurtMannequin { damage, knockback } => {
                    self.mannequin_hurt(id, ty, damage, by, Some(center));
                    self.deco_push_at(i, knockback);
                }
                DecoBlastEffect::Push(knockback) => self.deco_push_at(i, knockback),
                DecoBlastEffect::BreakStand(compound) => {
                    let pos = nbtio::f64s::<3>(compound.get("Pos")).unwrap_or_default();
                    self.stand_broken(&compound, pos.map(|v| v.floor() as i32));
                    self.deco_remove(id);
                    self.game_event_at("minecraft:entity_die", pos, None);
                }
                // Loot attributed to the author when the hit was allowed (creative: nothing).
                DecoBlastEffect::KillAttached { compound, credited } => {
                    self.attached_kill_by(id, &compound, if credited { by } else { None });
                }
            }
        }
    }

    /// What blast `blast` does to registry entry `i`, decided from its compound read in place
    /// (`None`: nothing, out of reach or ignored). Each mutating outcome carries what it needs.
    fn deco_blast_effect(&self, i: usize, blast: &DecoBlast) -> Option<DecoBlastEffect> {
        let entry = &self.deco.live[i];
        if entry.removed {
            return None;
        }
        let kind = entry.kind;
        let compound = self.live_nbt(i)?;
        let ignored = match kind {
            DKind::Stand => !blast.blocklike || flag(compound, "Invisible"),
            DKind::Mannequin => false,
            _ => blast.wet || !blast.blocklike,
        };
        if ignored {
            return None;
        }
        let reach = self.deco_blast_reach(compound, blast.center, blast.diameter)?;
        // The hit: refused for a mob without the `mob_griefing` rule, or invulnerable.
        let refused = blast.refused_by_rule || flag(compound, "Invulnerable");
        let living = matches!(kind, DKind::Stand | DKind::Mannequin);
        // A smashed stand or a killed attached decoration does not read its exposure: deciding
        // these first spares the rays cast towards its box (the blast's dominant cost).
        if kind == DKind::Stand && !refused && !flag(compound, "Marker") {
            return Some(DecoBlastEffect::BreakStand(compound.clone()));
        }
        if !living && !refused {
            return Some(DecoBlastEffect::KillAttached { compound: compound.clone(), credited: true });
        }
        let (to_eye, exposure) = self.deco_blast_exposure(kind, compound, &reach, blast.center);
        let dist = reach.dist;
        if living {
            // Still there (refused, marker, mannequin): pushed like any living entity, reduced
            // by the resistance read before any hurt (as the former copy did).
            let strength = (1.0 - dist) * exposure * (1.0 - self.deco_knockback_resistance(kind, compound));
            let knockback = crate::mobs::mth::normalize(to_eye).map(|v| v * strength);
            if kind == DKind::Mannequin {
                let power = (1.0 - dist) * exposure;
                let damage = ((power * power + power) / 2.0 * 7.0 * blast.diameter + 1.0) as f32;
                return Some(DecoBlastEffect::HurtMannequin { damage, knockback });
            }
            return Some(DecoBlastEffect::Push(knockback));
        }
        // Refused hit: a non-zero push still knocks the hung item off (fixed frame excepted).
        let to_eye_len = (to_eye[0] * to_eye[0] + to_eye[1] * to_eye[1] + to_eye[2] * to_eye[2]).sqrt();
        let pushed = to_eye_len >= f64::from(1.0e-5f32) && (1.0 - dist) * exposure != 0.0;
        if pushed && !(kind.frame() && flag(compound, "Fixed")) {
            return Some(DecoBlastEffect::KillAttached { compound: compound.clone(), credited: false });
        }
        None
    }

    /// Reach of a blast centred on `center` over a decoration (`None`: beyond `diameter`, or
    /// no box). Shared by the explosion and the wind burst, so their physics cannot drift
    /// apart (the former copy of every compound, [`Server::deco_explosion`] switched off, keeps
    /// its own reckoning, the reference of the equivalence tests).
    fn deco_blast_reach(&self, compound: &Nbt, center: [f64; 3], diameter: f64) -> Option<BlastReach> {
        let pos = nbtio::f64s::<3>(compound.get("Pos")).unwrap_or_default();
        let offset = [pos[0] - center[0], pos[1] - center[1], pos[2] - center[2]];
        let dist = (offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2]).sqrt() / diameter;
        // Distance before the box: a painting's box reads its variant's size.
        if dist > 1.0 {
            return None;
        }
        let bbox = self.deco_box(compound)?;
        Some(BlastReach { pos, dist, bbox })
    }

    /// From the blast's center to the decoration's eye, and the share of its box the center
    /// sees (rays cast to its box: called only when the outcome needs it).
    fn deco_blast_exposure(&self, kind: DKind, compound: &Nbt, reach: &BlastReach, center: [f64; 3]) -> ([f64; 3], f64) {
        let eye = self.deco_eye(kind, compound);
        let pos = reach.pos;
        let to_eye = [pos[0] - center[0], pos[1] + eye - center[1], pos[2] - center[2]];
        (to_eye, f64::from(self.seen_percent(center, &reach.bbox)))
    }

    /// Explosion knockback resistance of a living decoration (0 when it has none).
    fn deco_knockback_resistance(&self, kind: DKind, compound: &Nbt) -> f64 {
        self.nbt_attribute(compound, kind.name(), "minecraft:explosion_knockback_resistance").unwrap_or(0.0)
    }

    // ---------- Pushes (shulker, piston) ----------

    /// Decorations whose box touches `area`, with their kind; none when there is none (no
    /// allocation on the usual path). Each compound is reached from its registry entry (its
    /// chunk known) and its box built from the entry's kind; `RUSTCRAFT_OPT_DECOSCAN=0` looks
    /// each one up by identifier again (a walk of the registry per decoration) and reads its
    /// kind from the compound.
    pub(crate) fn deco_touching(&self, area: &Aabb, attached_only: bool) -> Vec<(i32, DKind)> {
        self.deco_touching_with(area, attached_only, deco_scan_indexed())
    }

    /// [`Server::deco_touching`] with the lookup chosen (`indexed`: from the registry entry).
    pub(crate) fn deco_touching_with(&self, area: &Aabb, attached_only: bool, indexed: bool) -> Vec<(i32, DKind)> {
        if self.deco.live.is_empty() {
            return Vec::new();
        }
        (0..self.deco.live.len())
            .filter(|&i| {
                let l = &self.deco.live[i];
                !l.removed && (!attached_only || l.kind.attached())
            })
            .filter(|&i| {
                if indexed {
                    let k = self.deco.live[i].kind;
                    self.live_nbt(i).is_some_and(|e| self.deco_box_of(k, e).intersects(area))
                } else {
                    self.deco_nbt(self.deco.live[i].id).and_then(|e| self.deco_box(e)).is_some_and(|b| b.intersects(area))
                }
            })
            .map(|i| (self.deco.live[i].id, self.deco.live[i].kind))
            .collect()
    }

    /// A non-zero move given to something attached to a block: it dies and drops its item
    /// (a fixed frame does not move). True if it fell.
    pub(crate) fn deco_attached_moved(&mut self, id: i32, d: [f64; 3]) -> bool {
        if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= 0.0 {
            return false;
        }
        let Some(e) = self.deco_nbt(id).cloned() else { return false };
        match kind_of(&e) {
            Some(k) if k.frame() && flag(&e, "Fixed") => false,
            Some(k) if k.attached() => self.attached_kill_by(id, &e, None),
            _ => false,
        }
    }

    /// An opening shulker's push on the decorations touching `area`: what hangs on a block
    /// falls with its item, an armor stand or a mannequin is moved with its collisions (a
    /// marker or weightless stand has no physics and stays).
    pub(crate) fn deco_shove(&mut self, area: &Aabb, d: [f64; 3]) {
        for (id, k) in self.deco_touching(area, false) {
            if k.attached() {
                self.deco_attached_moved(id, d);
            } else {
                self.living_shove(id, d);
            }
        }
    }

    /// Knockback alone of a burst (wind), × `mult`: armor stands (block-like bursts only,
    /// visible ones) and mannequins pushed, reduced by their explosion knockback resistance;
    /// what is hung or laid (block-like bursts only) knocked off by a non-zero push, a fixed frame
    /// excepted, its item dropped.
    pub(crate) fn deco_blast_push(&mut self, center: [f64; 3], radius: f32, mult: f64, blocklike: bool) {
        if radius < 1.0e-5 || self.deco.live.is_empty() {
            return;
        }
        let double = f64::from(radius * 2.0);
        // By registry index (its chunk known): one pass, whatever the number of decorations.
        for i in 0..self.deco.live.len() {
            let (id, k) = (self.deco.live[i].id, self.deco.live[i].kind);
            if self.deco.live[i].removed || (!blocklike && !matches!(k, DKind::Mannequin)) {
                continue;
            }
            let Some(e) = self.live_nbt(i) else { continue };
            if k == DKind::Stand && flag(e, "Invisible") {
                continue;
            }
            let Some(reach) = self.deco_blast_reach(e, center, double) else { continue };
            let (to_eye, exposure) = self.deco_blast_exposure(k, e, &reach, center);
            let living = matches!(k, DKind::Stand | DKind::Mannequin);
            let res = if living { self.deco_knockback_resistance(k, e) } else { 0.0 };
            let kp = (1.0 - reach.dist) * exposure * mult * (1.0 - res);
            let kb = crate::mobs::mth::normalize(to_eye).map(|v| v * kp);
            if living {
                self.deco_push_at(i, kb);
            } else if kb.iter().any(|&v| v != 0.0) && !(k.frame() && flag(e, "Fixed")) {
                let e = e.clone();
                self.attached_kill_by(id, &e, None);
            }
        }
    }

    // ---------- Living decorations in area effects ----------

    /// Armor stands and mannequins present, with their box (smash knockback, wind bursts).
    pub(crate) fn deco_living(&self) -> Vec<LivingDeco> {
        let mut v = Vec::new();
        for (i, l) in self.deco.live.iter().enumerate() {
            if l.removed || !matches!(l.kind, DKind::Stand | DKind::Mannequin) {
                continue;
            }
            let Some(e) = self.live_nbt(i) else { continue };
            let Some(bbox) = self.deco_box(e) else { continue };
            v.push(LivingDeco {
                id: l.id,
                uuid: nbtio::uuid(e.get("UUID")).unwrap_or(0),
                pos: nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default(),
                bbox,
                marker: l.kind == DKind::Stand && flag(e, "Marker"),
            });
        }
        v
    }

    /// Living decorations still in the world that transmit a waypoint, or that `keep` names (tracked
    /// by `/waypoint modify` without any range), handed to `f`. The range is only computed for one
    /// whose data or equipment can raise it from the default zero.
    pub(crate) fn deco_waypoint_transmitters(&self, keep: impl Fn(i32) -> bool, mut f: impl FnMut(DecoTransmitter<'_>)) {
        const RANGE: &str = crate::waypoints::TRANSMIT_RANGE;
        for (i, l) in self.deco.live.iter().enumerate() {
            if l.removed || !matches!(l.kind, DKind::Stand | DKind::Mannequin) {
                continue;
            }
            let Some(e) = self.live_nbt(i) else { continue };
            let transmit_range = if stand::attribute_may_differ(e, RANGE) {
                self.nbt_attribute(e, l.kind.name(), RANGE).unwrap_or(0.0)
            } else {
                0.0
            };
            if transmit_range > 0.0 || keep(l.id) {
                f(DecoTransmitter { id: l.id, ticked: l.ticked, transmit_range, compound: e });
            }
        }
    }

    /// Push of a living decoration: the velocity is added to its saved motion and goes to the
    /// client with its next tracking, which runs every tick (even without physics) and sends it
    /// only if it differs from the last one sent (beyond 1e-7 squared), like the game.
    pub(crate) fn deco_push(&mut self, id: i32, v: [f64; 3]) {
        if !v.iter().all(|x| x.is_finite()) {
            return;
        }
        // One registry search; the compound read and rewritten in place (its chunk known).
        let Some(i) = self.deco.live.iter().position(|l| l.id == id && !l.removed) else { return };
        self.push_live_entry(i, v);
    }

    /// [`Server::deco_push`] of registry entry `i`, whose index the caller already holds (no walk
    /// of the registry for its identifier: one per push made a blast over many stands quadratic).
    fn deco_push_at(&mut self, i: usize, v: [f64; 3]) {
        if !self.deco.switches.by_index {
            return self.deco_push(self.deco.live[i].id, v);
        }
        if v.iter().all(|x| x.is_finite()) && !self.deco.live[i].removed {
            self.push_live_entry(i, v);
        }
    }

    fn push_live_entry(&mut self, i: usize, v: [f64; 3]) {
        let Some(e) = self.live_nbt(i) else { return };
        let m = nbtio::motion(e.get("Motion")).unwrap_or_default();
        let (pos, ground) = (nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default(), flag(e, "OnGround"));
        let nv = [m[0] + v[0], m[1] + v[1], m[2] + v[2]];
        self.deco_edit_at(i, |n| nbtio::set(n, "Motion", dbl(nv)));
        // Not tracked yet (pushed before its first tick): the client knows the state before.
        let t = &mut self.deco.live[i].track;
        t.begin(pos, m, ground);
        t.needs_sync = true;
    }

    /// Value of an attribute of a living decoration (`None`: not one, or an attribute its type
    /// does not have).
    pub(crate) fn deco_attribute(&self, id: i32, name: &str) -> Option<f64> {
        let e = self.deco_nbt(id)?;
        let ty = e.get("id").and_then(Nbt::as_str)?;
        self.nbt_attribute(e, ty, name)
    }

    /// Saved entry of attribute `name` of a living decoration replaced (or added at the end of
    /// its list): base, then its modifiers if any (`/attribute`).
    pub(crate) fn deco_store_attribute(&mut self, id: i32, name: &str, base: f64, mods: &[(String, f64, i32)]) {
        let op = |o: i32| match o {
            1 => "add_multiplied_base",
            2 => "add_multiplied_total",
            _ => "add_value",
        };
        let mut entry = vec![("id".to_owned(), Nbt::String(name.to_owned())), ("base".to_owned(), Nbt::Double(base))];
        if !mods.is_empty() {
            let list = mods
                .iter()
                .map(|(m, v, o)| {
                    Nbt::Compound(vec![
                        ("id".to_owned(), Nbt::String(m.clone())),
                        ("amount".to_owned(), Nbt::Double(*v)),
                        ("operation".to_owned(), Nbt::String(op(*o).to_owned())),
                    ])
                })
                .collect();
            entry.push(("modifiers".to_owned(), Nbt::List(list)));
        }
        let entry = Nbt::Compound(entry);
        self.deco_edit(id, |n| {
            let mut list = nbtio::list(n.get("attributes")).to_vec();
            match list.iter_mut().find(|a| a.get("id").and_then(Nbt::as_str) == Some(name)) {
                Some(a) => *a = entry,
                None => list.push(entry),
            }
            nbtio::set(n, "attributes", Nbt::List(list));
        });
        // A synchronised attribute goes to the client (scale, armor…).
        if let Some(e) = self.deco_nbt(id)
            && let Some(ty) = e.get("id").and_then(Nbt::as_str)
            && client_visible(e)
        {
            let attributes = self.nbt_synced_attributes(e, ty, Some(name));
            if !attributes.is_empty() {
                let _ = self.send(Event::Attributes { entity: id, attributes });
            }
        }
    }

    // ---------- Player ----------

    /// Item that places an entity, used on a block; true if the item is one
    /// (even refused). `at`: absolute touched point.
    pub(crate) fn deco_use_on(&mut self, slot: usize, st: &Stack, clicked: P, face: Dir, at: [f64; 3]) -> bool {
        let r = match self.hanging_use_on(slot, st, clicked, face) {
            Some(r) => r,
            None => match self.stand_use_on(slot, st, clicked, face) {
                Some(r) => r,
                None => match self.cushion_use_on(slot, st, clicked, face, at) {
                    Some(r) => r,
                    None => return false,
                },
            },
        };
        // "Used" statistic for a successful or consumed use.
        if r != Use::Fail {
            self.award_stat("minecraft:used", &st.id, 1);
        }
        self.send_slot(slot);
        true
    }

    /// Entity removed by a command (`/kill`): leaves the registry.
    pub(crate) fn deco_forget(&mut self, id: i32) {
        for l in self.deco.live.iter_mut().filter(|l| l.id == id) {
            l.removed = true;
        }
    }

    /// Player hit on a decoration; true if the entity is one (handled).
    pub(crate) fn deco_attacked(&mut self, id: i32) -> bool {
        let Some(kind) = self.deco.live.iter().find(|l| l.id == id && !l.removed).map(|l| l.kind) else {
            return false;
        };
        if self.game_mode == 3 || self.life.vit.dead || !self.deco_in_reach(id) {
            return true;
        }
        match kind {
            DKind::Stand => self.stand_attacked(id),
            DKind::Mannequin => {}
            k => {
                // Hit at 0 if the block is reachable.
                let Some(e) = self.deco_nbt(id).cloned() else { return true };
                let pos = self.attached_pos(k, &e);
                if self.may_interact(pos) {
                    self.attached_hurt_by_player(id, k, &e);
                }
            }
        }
        true
    }

    /// Player right click on a decoration; true if the entity is one (handled).
    pub(crate) fn deco_interact(&mut self, id: i32, hand: i32, at: [f64; 3], secondary: bool) -> bool {
        let Some(kind) = self.deco.live.iter().find(|l| l.id == id && !l.removed).map(|l| l.kind) else {
            return false;
        };
        if self.life.vit.dead || !self.deco_in_reach(id) {
            return true;
        }
        let slot = if hand == 0 { 36 + self.held as usize } else { 45 };
        let used = match kind {
            DKind::Frame | DKind::GlowFrame if self.game_mode != 3 => self.frame_interact(id, kind, slot),
            DKind::Stand if self.game_mode != 3 => self.stand_interact(id, slot, at),
            DKind::Cushion if self.game_mode != 3 => self.cushion_interact(id, secondary),
            DKind::Knot if self.game_mode != 3 => self.knot_interact(id, slot),
            _ => false,
        };
        // Unconsumed click on a living entity: the held item acts on it
        // (the interaction with a mob); only the name tag concerns stand and mannequin.
        if !used && self.game_mode != 3 && matches!(kind, DKind::Stand | DKind::Mannequin) {
            self.deco_name_tag(id, slot);
        }
        true
    }

    /// Name tag on a stand or a mannequin: name set, tag consumed (except in
    /// creative), `entity_interact` event.
    fn deco_name_tag(&mut self, id: i32, slot: usize) {
        let Some(name) = self.slots[slot]
            .as_ref()
            .filter(|s| s.id == "minecraft:name_tag")
            .and_then(|s| crate::items::component(s, "minecraft:custom_name"))
            .cloned()
        else {
            return;
        };
        let Some(e) = self.deco_nbt(id) else { return };
        let pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        // Data sent only if it changes.
        if e.get("CustomName") != Some(&name) {
            let set = name.clone();
            self.deco_edit(id, |e| put_after(e, "UUID", "CustomName", set));
            self.deco_data(id, vec![(2, Meta::Text(Some(name)))]);
        }
        self.consume_held(slot);
        self.send_slot(slot);
        self.game_event_at("minecraft:entity_interact", pos, Some(crate::mobs::mob::Ent::Player));
    }

    /// Interaction range (margin of 3) from the eyes.
    fn deco_in_reach(&self, id: i32) -> bool {
        let Some(e) = self.deco_nbt(id) else { return false };
        let Some(b) = self.deco_box(e) else { return false };
        let range = self.player_attribute("minecraft:entity_interaction_range") + 3.0;
        let eye = [self.pos[0], self.pos[1] + self.ent_eye_height(crate::commands::ents::E::Player), self.pos[2]];
        let d2: f64 = (0..3)
            .map(|a| {
                let c = eye[a].clamp(b.min[a], b.max[a]);
                (eye[a] - c) * (eye[a] - c)
            })
            .sum();
        d2 < range * range
    }

    /// Eye height of a decoration of kind `k` (what an explosion pushes from): 0.85 of the height of
    /// what is hung or laid, the living sizes of the stands and mannequins.
    fn deco_eye(&self, k: DKind, e: &Nbt) -> f64 {
        match k {
            DKind::Painting => f64::from(0.5f32 * 0.85),
            DKind::Frame | DKind::GlowFrame => 0.0,
            DKind::Cushion => f64::from(0.25f32 * 0.85),
            DKind::Stand | DKind::Mannequin => self.living_size(e, k == DKind::Stand).2,
            DKind::Knot => f64::from(0.0625f32),
        }
    }

    /// Box of a decoration (selectors, range, overlaps).
    pub(crate) fn deco_box(&self, e: &Nbt) -> Option<Aabb> {
        Some(self.deco_box_of(kind_of(e)?, e))
    }

    /// [`Server::deco_box`] of a decoration whose kind is known (registry entry).
    fn deco_box_of(&self, k: DKind, e: &Nbt) -> Aabb {
        let p = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        let around = |w: f64, h: f64| Aabb { min: [p[0] - w / 2.0, p[1], p[2] - w / 2.0], max: [p[0] + w / 2.0, p[1] + h, p[2] + w / 2.0] };
        match k {
            DKind::Painting => {
                let (pos, d) = self.hang_place(k, e);
                let (w, h) = self.painting_size(e);
                painting_box(pos, d, w, h)
            }
            DKind::Frame | DKind::GlowFrame => {
                let (pos, d) = self.hang_place(k, e);
                frame_box(pos, d, hanging::has_map(e))
            }
            // Size of the marker, small stand, mannequin pose, scale.
            DKind::Stand | DKind::Mannequin => {
                let (w, h, _) = self.living_size(e, k == DKind::Stand);
                around(w, h)
            }
            DKind::Cushion => around(1.0, 0.25),
            DKind::Knot => around(0.375, 0.5),
        }
    }

    /// Lead knot hung on block `p`, if any.
    pub(crate) fn knot_at(&self, p: P) -> Option<i32> {
        self.deco
            .live
            .iter()
            .filter(|l| l.kind == DKind::Knot && !l.removed)
            .find(|l| self.deco_nbt_near(l.id, Some(l.chunk)).is_some_and(|e| block_pos_of(e) == p))
            .map(|l| l.id)
    }

    /// Block position of an attached entity (`pos`).
    fn attached_pos(&self, k: DKind, e: &Nbt) -> P {
        if k.hanging() {
            self.hang_place(k, e).0
        } else {
            block_pos_of(e)
        }
    }
}

/// Saved `block_pos` if it is less than 16 blocks from the position, otherwise the block of
/// the position.
pub(crate) fn block_pos_of(e: &Nbt) -> P {
    let pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
    let own = floor_pos(pos);
    match e.get("block_pos") {
        Some(Nbt::IntArray(a)) if a.len() == 3 => {
            let d: i64 = (0..3).map(|i| i64::from(a[i] - own[i]).pow(2)).sum();
            if (d as f64) < 256.0 { [a[0], a[1], a[2]] } else { own }
        }
        _ => own,
    }
}

/// Saved profile (name alone, or compound: name, id, properties, texture, cape, elytra,
/// model) in the network encoding of the resolvable profile (serializer 41): full profile if name
/// and id are there, otherwise partial; then the four optional appearance fields. `None`: unreadable
/// form or empty profile (default value, not sent).
pub(crate) fn profile_bytes(p: &Nbt) -> Option<Vec<u8>> {
    use rc_proto::Encode;
    use rc_proto::varint::write_varint;
    use rc_proto::write_str;
    let opt_str = |w: &mut Vec<u8>, s: Option<&str>| {
        s.is_some().encode(w);
        if let Some(s) = s {
            write_str(w, s);
        }
    };
    let ident = |s: &str| if s.contains(':') { s.to_owned() } else { format!("minecraft:{s}") };
    let (name, id) = match p {
        Nbt::String(n) => (Some(n.as_str()), None),
        Nbt::Compound(_) => (p.get("name").and_then(Nbt::as_str), nbtio::uuid(p.get("id"))),
        _ => return None,
    };
    let mut w = Vec::new();
    if let (Some(n), Some(u)) = (name, id) {
        true.encode(&mut w);
        w.extend_from_slice(&u.to_be_bytes());
        write_str(&mut w, n);
    } else {
        false.encode(&mut w);
        opt_str(&mut w, name);
        id.is_some().encode(&mut w);
        if let Some(u) = id {
            w.extend_from_slice(&u.to_be_bytes());
        }
    }
    // Properties: list of {name, value, signature?} or table name → values.
    let mut props: Vec<(&str, &str, Option<&str>)> = Vec::new();
    match p.get("properties") {
        Some(Nbt::List(l)) => {
            for q in l {
                if let (Some(n), Some(v)) = (q.get("name").and_then(Nbt::as_str), q.get("value").and_then(Nbt::as_str)) {
                    props.push((n, v, q.get("signature").and_then(Nbt::as_str)));
                }
            }
        }
        Some(Nbt::Compound(m)) => {
            for (n, vs) in m {
                props.extend(nbtio::list(Some(vs)).iter().filter_map(Nbt::as_str).map(|v| (n.as_str(), v, None)));
            }
        }
        _ => {}
    }
    write_varint(&mut w, props.len() as i32);
    for (n, v, s) in props {
        write_str(&mut w, n);
        write_str(&mut w, v);
        opt_str(&mut w, s);
    }
    for k in ["texture", "cape", "elytra"] {
        opt_str(&mut w, p.get(k).and_then(Nbt::as_str).map(ident).as_deref());
    }
    match p.get("model").and_then(Nbt::as_str) {
        Some(m @ ("slim" | "wide")) => {
            true.encode(&mut w);
            (m == "slim").encode(&mut w);
        }
        _ => false.encode(&mut w),
    }
    // Empty profile: equal to the mannequin's default.
    if w.iter().all(|&b| b == 0) { None } else { Some(w) }
}

/// Mannequin data: pose, arms, visible layers, immobility, description.
fn mannequin_meta(e: &Nbt, values: &mut Vec<(u8, Meta)>) {
    let pose = match e.get("pose").and_then(Nbt::as_str) {
        Some("fall_flying") => 1,
        Some("sleeping") => 2,
        Some("swimming") => 3,
        Some("crouching") => 5,
        _ => 0,
    };
    if pose != 0 {
        values.push((6, Meta::Pose(pose)));
    }
    if e.get("main_hand").and_then(Nbt::as_str) == Some("left") {
        values.push((15, Meta::Variant(0, 42)));
    }
    // Layers: all visible except those listed (cape, jacket, sleeves, legs, hat).
    let mut mask = 0x7Fi8;
    for h in nbtio::list(e.get("hidden_layers")) {
        let bit = match h.as_str() {
            Some("cape") => 0x01,
            Some("jacket") => 0x02,
            Some("left_sleeve") => 0x04,
            Some("right_sleeve") => 0x08,
            Some("left_pants_leg") => 0x10,
            Some("right_pants_leg") => 0x20,
            Some("hat") => 0x40,
            _ => 0,
        };
        mask &= !bit;
    }
    values.push((16, Meta::Byte(mask)));
    if let Some(b) = e.get("profile").and_then(profile_bytes) {
        values.push((17, Meta::Profile(b.into())));
    }
    if flag(e, "immovable") {
        values.push((18, Meta::Bool(true)));
    }
    if flag(e, "hide_description") {
        values.push((19, Meta::Text(None)));
    } else if let Some(d) = e.get("description") {
        values.push((19, Meta::Text(Some(d.clone()))));
    }
}
