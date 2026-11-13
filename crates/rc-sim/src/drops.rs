//! Items on the ground and experience orbs (`server-gameplay.md` § 1.3, 5.6): appearance (block
//! loot, thrown items, container contents), simple game physics (gravity, drag,
//! bounce), merging, pickup (`take_item_entity`), lifetime of 6000 ticks; loot of
//! broken blocks, tool wear, experience from ores.

use rc_client::{EntityEvent, Event};
use rc_physics::Aabb;
use rc_physics::body::{Body, Follow};
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::{AddEntity, Meta};

use crate::items::same;
use crate::menus::inventory_add;
use crate::mobs::combat::same_identifier;
use crate::mobs::recent::vibration::Src;
use crate::nbtio::int;
use crate::player::Stack;
use crate::server::Server;

const ITEM_SIZE: f64 = 0.25;
const ORB_SIZE: f64 = 0.5;
const LIFETIME: i32 = 6000;

#[cfg(test)]
thread_local! {
    /// Tests: forces `RUSTCRAFT_OPT_ITEM_RUNS=0` in this thread (equivalence of the two ways,
    /// `tests_l10order`), without touching the environment.
    pub(crate) static ONE_BY_ONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Tests: the switches of the ground items used by this thread (every combination of the
    /// equivalence proofs), without touching the environment.
    pub(crate) static FORCED_SWITCHES: std::cell::Cell<Option<DropSwitches>> = const { std::cell::Cell::new(None) };
    /// Tests: merge queries answered by the grid in this thread (proof that a test went that way).
    pub(crate) static GRID_QUERIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Tests: items read by the merges of this thread (the pairs tested, whatever the way): a
    /// count of the work that does not depend on the machine's load.
    pub(crate) static MERGE_PAIRS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// One more pair read by a merge (tests only, `MERGE_PAIRS`).
#[inline]
fn count_merge_pair() {
    #[cfg(test)]
    MERGE_PAIRS.with(|n| n.set(n.get() + 1));
}

/// Optimization switches of the ground items and orbs (`docs/perf.md`, ground items), each read
/// once at the first use: on by default, `=0` takes the former way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DropSwitches {
    /// `RUSTCRAFT_OPT_ITEM_MERGE_EARLY_EXIT`: an item that can never merge (pickup blocked,
    /// eternal, too old, full) skips its merge; otherwise its stack size is read once per merge
    /// and the distance is tested before the rest of each pair.
    pub merge_early_exit: bool,
    /// `RUSTCRAFT_OPT_ITEM_MERGE_LIVE_PREFILTER`: in a run, the distance is tested on the compact
    /// positions kept for the plates (`contact.live_items`) before an item is touched.
    pub merge_live_prefilter: bool,
    /// `RUSTCRAFT_OPT_ITEM_MERGE_GRID`: in a long run, the merge candidates come from a grid of
    /// the items by column (`ItemGrid`) instead of the whole list.
    pub merge_grid: bool,
    /// `RUSTCRAFT_OPT_ITEM_PICKUP_IN_PLACE`: a pickup stores the stack straight into the
    /// inventory instead of trying it first on a copy of the 46 slots.
    pub pickup_in_place: bool,
    /// `RUSTCRAFT_OPT_BODY_POOL`: the physics bodies of removed items and orbs are kept for the
    /// next ones instead of being freed and allocated again.
    pub body_pool: bool,
    /// `RUSTCRAFT_OPT_ENTITY_SAVE_LENT`: a saved entity chunk encodes the entities kept as read
    /// from their own list instead of from clones.
    pub save_lent: bool,
}

impl DropSwitches {
    /// Every optimization on (the default).
    #[cfg(test)]
    pub(crate) const ALL_ON: DropSwitches = DropSwitches {
        merge_early_exit: true,
        merge_live_prefilter: true,
        merge_grid: true,
        pickup_in_place: true,
        body_pool: true,
        save_lent: true,
    };
    /// Every optimization off (the former code).
    #[cfg(test)]
    pub(crate) const ALL_OFF: DropSwitches = DropSwitches {
        merge_early_exit: false,
        merge_live_prefilter: false,
        merge_grid: false,
        pickup_in_place: false,
        body_pool: false,
        save_lent: false,
    };

    pub(crate) fn current() -> DropSwitches {
        #[cfg(test)]
        if let Some(forced) = FORCED_SWITCHES.with(std::cell::Cell::get) {
            return forced;
        }
        static SWITCHES: std::sync::OnceLock<DropSwitches> = std::sync::OnceLock::new();
        *SWITCHES.get_or_init(|| DropSwitches {
            merge_early_exit: rc_data::opt_switch!("RUSTCRAFT_OPT_ITEM_MERGE_EARLY_EXIT"),
            merge_live_prefilter: rc_data::opt_switch!("RUSTCRAFT_OPT_ITEM_MERGE_LIVE_PREFILTER"),
            merge_grid: rc_data::opt_switch!("RUSTCRAFT_OPT_ITEM_MERGE_GRID"),
            pickup_in_place: rc_data::opt_switch!("RUSTCRAFT_OPT_ITEM_PICKUP_IN_PLACE"),
            body_pool: rc_data::opt_switch!("RUSTCRAFT_OPT_BODY_POOL"),
            save_lent: rc_data::opt_switch!("RUSTCRAFT_OPT_ENTITY_SAVE_LENT"),
        })
    }
}

/// `RUSTCRAFT_OPT_ITEM_RUNS=N` (default 8): in the entity walk, a run of items or orbs of at least
/// 1/N of their list ticks with the list taken out once; a shorter one, one by one out of its slot.
/// `0`: always one by one (the first version of the walk). Read once.
fn item_run_divisor() -> usize {
    #[cfg(test)]
    if ONE_BY_ONE.with(std::cell::Cell::get) {
        return 0;
    }
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_ITEM_RUNS").and_then(|v| v.parse().ok()).unwrap_or(8))
}

/// The run from `start` to `stop` of a list of `len` entities ticks one by one.
fn one_by_one(start: usize, stop: usize, len: usize) -> bool {
    let n = item_run_divisor();
    n == 0 || (stop - start) * n < len
}

/// Bodies of removed items and orbs kept for the next ones (about 300 bytes each): enough for a
/// farm's births of a tick, bounded after a big wave of removals.
const SPARE_BODIES: usize = 1024;

/// Physics of a server item or orb (authoritative instance).
fn server_body(pos: [f64; 3], vel: [f64; 3], size: f64, eye: f64) -> Box<Body> {
    let mut b = Box::new(Body::new(pos, vel, size, size, eye));
    b.server = true;
    b
}

#[derive(Debug, Clone)]
pub struct ItemEnt {
    pub id: i32,
    pub stack: Stack,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub on_ground: bool,
    pub age: i32,
    pub pickup_delay: i32,
    pub ticks: i32,
    /// Changed block during this tick (merge every 2 ticks, otherwise 40).
    pub moved: bool,
    /// Network tracking (S4: official server rules, interval 20).
    pub(crate) track: crate::mobs::track::Tracker,
    removed: bool,
    /// Health points (`Health`, 5 at the start): `/damage` removes them.
    pub health: i32,
    /// Physics shared with the client (`rc_physics::body`).
    pub body: Box<Body>,
    /// W8c: thrower (`Owner`: the player who threw it, the allay who threw it) — triggers
    /// `thrown_item_picked_up_by_*`.
    pub thrower: Option<crate::mobs::mob::Ent>,
    /// Yaw drawn at creation (random, never changed afterwards): it rotates the velocity at the
    /// passage of an End portal.
    pub yaw: f32,
    /// G5-F05: fire ticks left (`Fire`), and the "on fire" flag last sent to the client.
    pub fire: i32,
    fire_flag: bool,
    /// Rank in the entity walk (`walk::next_rank`), and "ticked this tick" (only a ticked item
    /// goes through a portal, `drops_portal_tick`).
    pub(crate) rank: u64,
    pub(crate) ticked: bool,
    /// L10-ENT: entity fields kept and saved like the game's (`Air`, `Invulnerable`).
    pub common: Common,
}

/// Common entity fields of a ground item or an orb: the air supply (300, a value read from the
/// NBT is kept), the permanent invulnerability and the ticks of temporary invulnerability left
/// (an item dropped by a cushion struck by lightning: 20), counted down at each tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Common {
    /// Kept as the short the game saves it as: the three fields fit in 8 bytes, which keeps the
    /// item and orb entries walked every tick from growing.
    pub air: i16,
    pub invulnerable: bool,
    pub invulnerable_time: i32,
}

impl Default for Common {
    fn default() -> Common {
        Common { air: 300, invulnerable: false, invulnerable_time: 0 }
    }
}

impl Common {
    /// Start of the entity's tick: one tick less of temporary invulnerability.
    fn count_down(&mut self) {
        if self.invulnerable_time > 0 {
            self.invulnerable_time -= 1;
        }
    }
}

/// Common entity part of an item or orb compound, read like the game's load: fire ticks, ground
/// flag, fall distance, kept fields (the portal delay is restored by its caller).
fn read_common(n: &Nbt) -> (i32, bool, f64, Common) {
    let fire = int(n, "Fire").unwrap_or(0) as i16 as i32;
    let on_ground = int(n, "OnGround").is_some_and(|v| v != 0);
    let fall = n.get("fall_distance").and_then(crate::mobs::mob::f64_of).unwrap_or(0.0);
    let air = int(n, "Air").map_or(300, |v| v as i16);
    let invulnerable = int(n, "Invulnerable").is_some_and(|v| v != 0);
    let invulnerable_time = int(n, "invulnerable_time").map_or(0, |v| v as i32);
    (fire, on_ground, fall, Common { air, invulnerable, invulnerable_time })
}

/// Common entity part of an item or an orb as the game writes it.
struct Part {
    pos: [f64; 3],
    vel: [f64; 3],
    yaw: f32,
    fall: f64,
    fire: i32,
    on_ground: bool,
    common: Common,
    cooldown: i32,
}

/// The game's write of the common entity part (without the UUID), in its order.
fn write_common(f: &mut Vec<(String, Nbt)>, p: Part) {
    let dbl = |v: [f64; 3]| Nbt::List(v.iter().map(|&x| Nbt::Double(x)).collect());
    f.push(("Pos".into(), dbl(p.pos)));
    f.push(("Motion".into(), dbl(p.vel)));
    f.push(("Rotation".into(), Nbt::List(vec![Nbt::Float(p.yaw), Nbt::Float(0.0)])));
    f.push(("fall_distance".into(), Nbt::Double(p.fall)));
    f.push(("Fire".into(), Nbt::Short(p.fire as i16)));
    f.push(("Air".into(), Nbt::Short(p.common.air)));
    f.push(("OnGround".into(), Nbt::Byte(i8::from(p.on_ground))));
    f.push(("Invulnerable".into(), Nbt::Byte(i8::from(p.common.invulnerable))));
    f.push(("PortalCooldown".into(), Nbt::Int(p.cooldown)));
    if p.common.invulnerable_time > 0 {
        f.push(("invulnerable_time".into(), Nbt::Int(p.common.invulnerable_time)));
    }
}

/// UUID shown and saved for a ground item or an orb (not kept from the world: derived from its
/// identifier).
pub(crate) fn ground_uuid(id: i32) -> u128 {
    GROUND_UUID_PREFIX | id as u32 as u128
}

const GROUND_UUID_PREFIX: u128 = 0x5332_0000_0000_0000_0000_0000_0000_0000;

/// Identifier whose [`ground_uuid`] is `u`, if `u` has that form.
pub(crate) fn ground_id_of_uuid(u: u128) -> Option<i32> {
    (u & !u128::from(u32::MAX) == GROUND_UUID_PREFIX).then_some(u as u32 as i32)
}

impl ItemEnt {
    /// Stand-in left in the slot of the item being ticked (removed: nothing counts or finds it).
    fn placeholder() -> ItemEnt {
        ItemEnt {
            id: i32::MIN,
            stack: Stack { id: String::new(), count: 0, components: None },
            pos: [0.0; 3],
            vel: [0.0; 3],
            on_ground: false,
            age: 0,
            pickup_delay: 0,
            ticks: 0,
            moved: false,
            track: Default::default(),
            removed: true,
            health: 0,
            body: server_body([0.0; 3], [0.0; 3], ITEM_SIZE, 0.2125),
            thrower: None,
            yaw: 0.0,
            fire: 0,
            fire_flag: false,
            rank: 0,
            ticked: false,
            common: Common::default(),
        }
    }

    /// Network tracking restarted from zero (item re-announced elsewhere, S6).
    pub(crate) fn retrack(&mut self) {
        self.track = Default::default();
    }
}

#[derive(Debug, Clone)]
pub struct Orb {
    pub id: i32,
    pub value: i32,
    pub count: i32,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub on_ground: bool,
    pub age: i32,
    pub ticks: i32,
    /// Network tracking (S4: official server rules, interval 20).
    track: crate::mobs::track::Tracker,
    removed: bool,
    /// Health points (`Health`, 5 at the start): `/damage` removes them.
    pub health: i32,
    /// Physics shared with the client (`rc_physics::body`).
    pub body: Box<Body>,
    /// G5-F05: fire ticks left, and the "on fire" flag last sent to the client.
    pub fire: i32,
    fire_flag: bool,
    /// Rank in the entity walk (`walk::next_rank`).
    pub(crate) rank: u64,
    /// L10-ENT: `Air` and `Invulnerable` kept and saved like the game's.
    pub common: Common,
}

impl Orb {
    /// Stand-in left in the slot of the orb being ticked (removed: nothing counts or absorbs it).
    fn placeholder() -> Orb {
        Orb {
            id: i32::MIN,
            value: 0,
            count: 0,
            pos: [0.0; 3],
            vel: [0.0; 3],
            on_ground: false,
            age: 0,
            ticks: 0,
            track: Default::default(),
            removed: true,
            health: 0,
            body: server_body([0.0; 3], [0.0; 3], ORB_SIZE, 0.425),
            fire: 0,
            fire_flag: false,
            rank: 0,
            common: Common::default(),
        }
    }
}

/// Split of a reward into orbs (values of the game).
pub fn orb_value(n: i32) -> i32 {
    for v in [2477, 1237, 617, 307, 149, 73, 37, 17, 7, 3] {
        if n >= v {
            return v;
        }
    }
    1
}

/// Experience dropped by a block broken with the right tool: ores (drawn range), sculk blocks
/// (constant, no draw), all cancelled by Silk Touch; mob spawner:
/// 15 + n(15) + n(15), Silk Touch included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockXp {
    Range(i32, i32),
    Const(i32),
    Spawner,
}

pub(crate) fn block_xp(name: &str) -> Option<BlockXp> {
    Some(match name.trim_start_matches("minecraft:") {
        "coal_ore" | "deepslate_coal_ore" => BlockXp::Range(0, 2),
        "nether_gold_ore" => BlockXp::Range(0, 1),
        "lapis_ore" | "deepslate_lapis_ore" | "nether_quartz_ore" => BlockXp::Range(2, 5),
        "diamond_ore" | "deepslate_diamond_ore" | "emerald_ore" | "deepslate_emerald_ore" => BlockXp::Range(3, 7),
        "redstone_ore" | "deepslate_redstone_ore" => BlockXp::Range(1, 5),
        "sculk" => BlockXp::Const(1),
        "sculk_sensor" | "calibrated_sculk_sensor" | "sculk_shrieker" | "sculk_catalyst" => BlockXp::Const(5),
        "spawner" => BlockXp::Spawner,
        _ => return None,
    })
}

/// A ground item of this id dampens the vibrations it makes (wool and its carpets, slabs and
/// stairs; moss carpets do not).
fn dampens_vibrations(id: &str) -> bool {
    id.ends_with("_wool")
        || (id.ends_with("_carpet") && !id.contains("moss"))
        || id.ends_with("_wool_slab")
        || id.ends_with("_wool_stairs")
}

fn bbox(pos: [f64; 3], size: f64) -> Aabb {
    let h = size / 2.0;
    Aabb { min: [pos[0] - h, pos[1], pos[2] - h], max: [pos[0] + h, pos[1] + size, pos[2] + h] }
}

/// Boxes that overlap (touching faces do not count).
fn crosses(a: &Aabb, b: &Aabb) -> bool {
    (0..3).all(|i| a.min[i] < b.max[i] && a.max[i] > b.min[i])
}

/// The game's merge reach of a ground item at `pos`: its box inflated by 0.5 horizontally. A
/// neighbour merges only if its own box crosses it (`crosses`: boxes that only touch do not).
fn merge_reach(pos: [f64; 3]) -> Aabb {
    let b = bbox(pos, ITEM_SIZE);
    Aabb { min: [b.min[0] - 0.5, b.min[1], b.min[2] - 0.5], max: [b.max[0] + 0.5, b.max[1], b.max[2] + 0.5] }
}

/// A ground item that may merge, for a stack size of `max`: neither pickup blocked (32767)
/// nor eternal (age −32768), not too old, not full.
fn may_merge(e: &ItemEnt, max: i32) -> bool {
    !e.removed && e.pickup_delay != 32767 && e.age != -32768 && e.age < LIFETIME && e.stack.count < max
}

/// Slot `j` of a run's list whose slot `i` is taken out (`left`: the slots before it, `right`:
/// those after).
fn run_slot<'a>(left: &'a mut [ItemEnt], right: &'a mut [ItemEnt], i: usize, j: usize) -> &'a mut ItemEnt {
    if j < i { &mut left[j] } else { &mut right[j - i - 1] }
}

/// Length of a run's list from which the merge candidates come from the grid: below, testing
/// the compact positions of every slot costs about as much as building the grid.
const ITEM_GRID_FROM: usize = 64;

/// Merge candidates of a long run of items (`RUSTCRAFT_OPT_ITEM_MERGE_GRID`): the slots of the
/// run's list by column of 2 × 2 blocks, read from the live positions of the plates
/// (`contact.live_items`). Built at the run's first due merge, then it follows the ticked item
/// when it changes column (`item_seen`) or goes: nothing else moves an item of a run, its list
/// is taken out. A column holds its slots in any order; the candidates are sorted back into list
/// order, the order the merges of a stack depend on.
#[derive(Debug, Default)]
pub(crate) struct ItemGrid {
    /// Built for the run in progress.
    built: bool,
    columns: hashbrown::HashMap<(i32, i32), Vec<u32>>,
    /// Column of each slot of the run's list, `None` once the item is gone.
    column_of: Vec<Option<(i32, i32)>>,
    /// Slots found by the last `near` (buffer kept from one query to the next).
    candidates: Vec<u32>,
}

impl ItemGrid {
    /// Column of an x or z coordinate: 2 blocks wide, so the candidates of a merge (within 0.75,
    /// plus a margin for rounding) span at most two columns on each axis.
    fn column(c: f64) -> i32 {
        (c * 0.5).floor() as i32
    }

    /// For a new run: built again at its first due merge.
    pub(crate) fn reset(&mut self) {
        self.built = false;
    }

    fn build(&mut self, live: &[([f64; 3], bool)]) {
        // Columns kept with their buffers from one run to the next; a map grown far beyond the
        // run's items (a scattered list before) is dropped.
        if self.columns.len() > 2 * live.len() + 64 {
            self.columns.clear();
        }
        for slots in self.columns.values_mut() {
            slots.clear();
        }
        self.column_of.clear();
        for (j, &(pos, gone)) in live.iter().enumerate() {
            let column = (!gone).then(|| (Self::column(pos[0]), Self::column(pos[2])));
            if let Some(c) = column {
                self.columns.entry(c).or_default().push(j as u32);
            }
            self.column_of.push(column);
        }
        self.built = true;
    }

    /// The item of slot `j` is now at `pos`, or gone.
    fn moved(&mut self, j: usize, pos: [f64; 3], gone: bool) {
        if !self.built {
            return;
        }
        let new = (!gone).then(|| (Self::column(pos[0]), Self::column(pos[2])));
        let Some(old) = self.column_of.get_mut(j).map(|c| std::mem::replace(c, new)) else { return };
        if old == new {
            return;
        }
        if let Some(slots) = old.and_then(|c| self.columns.get_mut(&c))
            && let Some(k) = slots.iter().position(|&s| s as usize == j)
        {
            slots.swap_remove(k);
        }
        if let Some(c) = new {
            self.columns.entry(c).or_default().push(j as u32);
        }
    }

    /// Slots whose live position puts their box across `reach`, in list order, into
    /// `candidates`; returns their number.
    fn near(&mut self, reach: &Aabb, live: &[([f64; 3], bool)]) -> usize {
        #[cfg(test)]
        GRID_QUERIES.with(|n| n.set(n.get() + 1));
        self.candidates.clear();
        // A crossing box has its center within half its width (0.125) of the reach: a whole item
        // size leaves room for rounding.
        let margin = ITEM_SIZE;
        let (x0, x1) = (Self::column(reach.min[0] - margin), Self::column(reach.max[0] + margin));
        let (z0, z1) = (Self::column(reach.min[2] - margin), Self::column(reach.max[2] + margin));
        for cx in x0..=x1 {
            for cz in z0..=z1 {
                let Some(slots) = self.columns.get(&(cx, cz)) else { continue };
                let crossing = |&&j: &&u32| live.get(j as usize).is_some_and(|l| crosses(&bbox(l.0, ITEM_SIZE), reach));
                self.candidates.extend(slots.iter().filter(crossing));
            }
        }
        self.candidates.sort_unstable();
        self.candidates.len()
    }
}

/// G5-F05: orb `i` absorbs the orbs whose box crosses its own inflated by 0.5, of the same value
/// and with an identifier congruent modulo 40: counts added, youngest age kept, the others removed.
/// `candidates`: indices to test (all, or those of its residue modulo 40).
pub(crate) fn merge_orbs(
    orbs: &mut [Orb],
    i: usize,
    live: &mut [([f64; 3], bool)],
    candidates: impl Iterator<Item = usize>,
) {
    let (id, value) = (orbs[i].id, orbs[i].value);
    let b = bbox(orbs[i].pos, ORB_SIZE);
    let zone = Aabb { min: b.min.map(|c| c - 0.5), max: b.max.map(|c| c + 0.5) };
    let (mut count, mut age) = (orbs[i].count, orbs[i].age);
    for j in candidates {
        let o = &mut orbs[j];
        if j == i || o.removed || (o.id - id) % 40 != 0 || o.value != value || !crosses(&bbox(o.pos, ORB_SIZE), &zone) {
            continue;
        }
        count += o.count;
        age = age.min(o.age);
        o.removed = true;
        if let Some(l) = live.get_mut(j) {
            l.1 = true;
        }
    }
    (orbs[i].count, orbs[i].age) = (count, age);
}

/// Orb count from which the merge candidates are bucketed (bench `g5f05_bench`: whole list
/// cheaper at 20 orbs, buckets ahead at 100).
const ORB_BUCKETS_FROM: usize = 40;

/// Orbs still present by residue of their identifier modulo 40, in list order.
pub(crate) fn orb_buckets(orbs: &[Orb], buckets: &mut Vec<Vec<u32>>) {
    buckets.resize_with(40, Vec::new);
    for b in buckets.iter_mut() {
        b.clear();
    }
    for (j, o) in orbs.iter().enumerate().filter(|(_, o)| !o.removed) {
        buckets[o.id.rem_euclid(40) as usize].push(j as u32);
    }
}

/// True when the "on fire" point of an item or orb falls this tick (counter on a multiple of 20).
fn burn_due(fire: i32, immune: bool) -> bool {
    fire > 0 && !immune && fire % 20 == 0
}

/// Fire counter at the start of the tick of an item or orb: an immune one is put out, otherwise
/// the counter goes down. True if the "on fire" point falls now (never in lava).
fn burn_step(fire: &mut i32, immune: bool, in_lava: bool) -> bool {
    if *fire <= 0 {
        return false;
    }
    if immune {
        *fire = 0;
        return false;
    }
    let hurt = *fire % 20 == 0 && !in_lava;
    *fire -= 1;
    hurt
}

impl Server {
    fn entity_kind(&self, name: &str) -> i32 {
        self.entity_kind_id(name)
    }

    fn new_entity_id(&mut self) -> i32 {
        self.next_entity += 1;
        self.next_entity
    }

    /// Item entity created and announced to the client.
    pub(crate) fn spawn_item(&mut self, pos: [f64; 3], vel: [f64; 3], stack: Stack, pickup_delay: i32) {
        if stack.count <= 0 {
            return;
        }
        if crate::life::s2log() {
            eprintln!("singleplayer: ground item {} ×{} at {pos:?} (delay {pickup_delay})", stack.id, stack.count);
        }
        let id = self.new_entity_id();
        let yaw = self.life.rng.next_float() * 360.0;
        let add = AddEntity {
            id,
            uuid: (self.life.rng.next_long() as u128) << 64 | self.life.rng.next_long() as u64 as u128,
            kind: self.entity_kind("minecraft:item"),
            pos,
            velocity: vel,
            pitch: 0.0,
            yaw,
            head_yaw: 0.0,
            data: 0,
        };
        let _ = self.send(Event::Entity(EntityEvent::Add(Box::new(add))));
        let net = self.net(Some(&stack));
        let _ = self.send(Event::Entity(EntityEvent::Data { id, values: vec![(8, Meta::Item(net))] }));
        let body = self.new_body(pos, vel, ITEM_SIZE, 0.2125);
        self.life.drops.push(ItemEnt {
            id,
            stack,
            pos,
            vel,
            on_ground: false,
            age: 0,
            pickup_delay,
            ticks: 0,
            moved: false,
            track: Default::default(),
            removed: false,
            health: 5,
            body,
            thrower: None,
            yaw,
            fire: 0,
            fire_flag: false,
            rank: crate::walk::next_rank(),
            ticked: false,
            common: Common::default(),
        });
    }

    /// W8c: thrower of the last item placed on the ground.
    pub(crate) fn set_last_thrower(&mut self, who: crate::mobs::mob::Ent) {
        if let Some(d) = self.life.drops.last_mut() {
            d.thrower = Some(who);
        }
    }

    /// W8c: a mob picks up an item: thrown by the player →
    /// `thrown_item_picked_up_by_entity` (whole stack from before the pickup).
    pub(crate) fn thrown_pickup_by_mob(&mut self, item: i32, m: &crate::mobs::mob::Mob) {
        let Some(d) = self.life.drops.iter().find(|d| d.id == item && !d.removed) else { return };
        if d.thrower == Some(crate::mobs::mob::Ent::Player) {
            let s = d.stack.clone();
            self.trig_picked_up(
                "minecraft:thrown_item_picked_up_by_entity",
                &s,
                Some(crate::progress::pred::Who::Mob(m)),
            );
        }
    }

    /// Random initial velocity.
    pub(crate) fn spawn_item_random(&mut self, pos: [f64; 3], stack: Stack, delay: i32) {
        let vx = self.life.rng.next_double() * 0.2 - 0.1;
        let vz = self.life.rng.next_double() * 0.2 - 0.1;
        self.spawn_item(pos, [vx, 0.2, vz], stack, delay);
    }

    /// Loot placed at the center of a block, if the `block_drops` rule allows it.
    pub(crate) fn pop_resource(&mut self, p: [i32; 3], stack: Stack) {
        if !self.rule("minecraft:block_drops", true) {
            return;
        }
        let r = &mut self.life.rng;
        let mut off = || r.next_double() * 0.5 - 0.25;
        let x = f64::from(p[0]) + 0.5 + off();
        let y = f64::from(p[1]) + 0.5 + off() - ITEM_SIZE / 2.0;
        let z = f64::from(p[2]) + 0.5 + off();
        self.spawn_item_random([x, y, z], stack, 10);
    }

    /// Contents of a scattered container.
    pub(crate) fn drop_contents(&mut self, at: [f64; 3], mut stack: Stack) {
        let range = 1.0 - ITEM_SIZE;
        let x = at[0].floor() + self.life.rng.next_double() * range + ITEM_SIZE / 2.0;
        let y = at[1].floor() + self.life.rng.next_double() * range;
        let z = at[2].floor() + self.life.rng.next_double() * range + ITEM_SIZE / 2.0;
        while stack.count > 0 {
            let n = (self.life.rng.next_int_bound(21) + 10).min(stack.count);
            let part = Stack { count: n, ..stack.clone() };
            stack.count -= n;
            let s = 0.114_850_001_711_398_36;
            let v = [self.life.rng.triangle(0.0, s), self.life.rng.triangle(0.2, s), self.life.rng.triangle(0.0, s)];
            self.spawn_item([x, y, z], v, part, 0);
        }
    }

    pub(crate) fn spawn_item_at(&mut self, at: [f64; 3], stack: Stack) {
        self.drop_contents(at, stack);
    }

    /// Item thrown by the player: in front of them, or at random (death).
    pub(crate) fn drop_from_player_with(&mut self, stack: Stack, randomly: bool) {
        let eye = self.player_eye_h();
        let pos = [self.pos[0], self.pos[1] + eye - 0.3, self.pos[2]];
        let r = &mut self.life.rng;
        let vel = if randomly {
            let pow = r.next_float() * 0.5;
            let dir = r.next_float() * std::f32::consts::TAU;
            [f64::from(-dir.sin() * pow), 0.2, f64::from(dir.cos() * pow)]
        } else {
            let (sx, cx) = (self.pitch.to_radians().sin(), self.pitch.to_radians().cos());
            let (sy, cy) = (self.yaw.to_radians().sin(), self.yaw.to_radians().cos());
            let dir = r.next_float() * std::f32::consts::TAU;
            let pow2 = 0.02 * r.next_float();
            let jitter = (r.next_float() - r.next_float()) * 0.1;
            [
                f64::from(-sy * cx * 0.3) + f64::from(dir).cos() * f64::from(pow2),
                f64::from(-sx * 0.3 + 0.1 + jitter),
                f64::from(cy * cx * 0.3) + f64::from(dir).sin() * f64::from(pow2),
            ]
        };
        self.spawn_item(pos, vel, stack, 40);
        if !randomly {
            self.set_last_thrower(crate::mobs::mob::Ent::Player); // W8c: thrown by hand
        }
    }

    pub(crate) fn drop_from_player(&mut self, stack: Stack, _thrown: bool) {
        self.drop_from_player_with(stack, false);
    }

    /// Experience orbs.
    pub(crate) fn spawn_orbs(&mut self, pos: [f64; 3], mut amount: i32) {
        while amount > 0 {
            let v = orb_value(amount);
            amount -= v;
            let r = self.life.rng.next_int_bound(40);
            // G5-F05: box of 1 around the point, crossing the orb's box (strict bounds).
            let around = Aabb { min: pos.map(|c| c - 0.5), max: pos.map(|c| c + 0.5) };
            if let Some(o) = self.life.orbs.iter_mut().find(|o| {
                !o.removed && o.value == v && (o.id - r) % 40 == 0 && crosses(&bbox(o.pos, ORB_SIZE), &around)
            }) {
                o.count += 1;
                o.age = 0;
                continue;
            }
            self.add_orb(pos, v);
        }
    }

    /// A new orb's spot moved out of what it is stuck in: when its box is not free (a test made for
    /// no entity: blocks and solid entities), its centre goes to the closest free point of a cube of
    /// its size around it, among the blocks inside the border; it stays put when there is none.
    fn orb_spawn_out_of_blocks(&self, pos: [f64; 3]) -> [f64; 3] {
        if self.room_free(crate::solid_entities::RoomFor::NoEntity, &bbox(pos, ORB_SIZE)) {
            return pos;
        }
        let half = ORB_SIZE / 2.0;
        let center = [pos[0], pos[1] + half, pos[2]];
        let allowed = Aabb { min: center.map(|c| c - half), max: center.map(|c| c + half) };
        self.find_free_position(&allowed, center, [ORB_SIZE; 3]).map_or(pos, |c| [c[0], c[1] - half, c[2]])
    }

    /// A single orb of this value, no split or merge (reward of a trade).
    pub(crate) fn add_orb(&mut self, pos: [f64; 3], v: i32) {
        self.add_orb_dir(pos, [0.0; 3], v);
    }

    /// Orb thrown roughly towards `dir` (S8: the experience orb):
    /// random velocity flipped if it opposes `dir`, start offset by half a size.
    pub(crate) fn add_orb_dir(&mut self, pos: [f64; 3], dir: [f64; 3], v: i32) {
        let id = self.new_entity_id();
        let rr = &mut self.life.rng;
        let mut vel =
            [(rr.next_double() * 0.2 - 0.1) * 2.0, rr.next_double() * 0.2 * 2.0, (rr.next_double() * 0.2 - 0.1) * 2.0];
        let l2 = dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2];
        let mut pos = pos;
        if l2 > 0.0 {
            if dir[0] * vel[0] + dir[1] * vel[1] + dir[2] * vel[2] < 0.0 {
                vel = vel.map(|c| -c);
            }
            let l = l2.sqrt();
            pos = std::array::from_fn(|i| pos[i] + dir[i] / l * ORB_SIZE * 0.5);
        }
        let pos = self.orb_spawn_out_of_blocks(pos);
        let rr = &mut self.life.rng;
        let uuid = (rr.next_long() as u128) << 64 | rr.next_long() as u64 as u128;
        for e in self.orb_announcement(id, uuid, pos, vel, v) {
            let _ = self.send(e);
        }
        let body = self.new_body(pos, vel, ORB_SIZE, 0.425);
        self.life.orbs.push(Orb {
            id,
            value: v,
            count: 1,
            pos,
            vel,
            on_ground: false,
            age: 0,
            ticks: 0,
            track: Default::default(),
            removed: false,
            health: 5,
            body,
            fire: 0,
            fire_flag: false,
            rank: crate::walk::next_rank(),
            common: Common::default(),
        });
        self.orb_joined();
    }

    /// The orb just pushed at the end of the list, during a walk whose merge buckets are built:
    /// it joins its bucket, at the end like in the list (the merges later in this walk see it,
    /// as the linear way does). List taken out for a run: buckets rebuilt at the next merge.
    fn orb_joined(&mut self) {
        if !self.walk.buckets_built {
            return;
        }
        let (Some(o), false) = (self.life.orbs.last(), self.contact.orbs_taken) else {
            self.walk.buckets_built = false;
            return;
        };
        let (j, r) = (self.life.orbs.len() - 1, o.id.rem_euclid(40) as usize);
        match self.contact.orb_buckets.get_mut(r) {
            Some(b) => b.push(j as u32),
            None => self.walk.buckets_built = false,
        }
    }

    /// Orb summoned (`/summon`) or read with a chunk: value, count, age, health points,
    /// velocity and the common entity fields from the NBT, like the game's load.
    pub(crate) fn summon_orb(&mut self, pos: [f64; 3], nbt: Option<&Nbt>) -> i32 {
        let get = |k: &str, d: i64| nbt.and_then(|n| int(n, k)).unwrap_or(d) as i32;
        let (value, count, age, health) = (get("Value", 0), get("Count", 1).max(1), get("Age", 0), get("Health", 5));
        let vel = nbt.and_then(|n| crate::nbtio::motion(n.get("Motion"))).unwrap_or_default();
        let id = self.new_entity_id();
        let rr = &mut self.life.rng;
        let uuid = (rr.next_long() as u128) << 64 | rr.next_long() as u64 as u128;
        for e in self.orb_announcement(id, uuid, pos, vel, value) {
            let _ = self.send(e);
        }
        let (fire, on_ground, fall, common) = nbt.map(read_common).unwrap_or_default();
        let mut body = self.new_body(pos, vel, ORB_SIZE, 0.425);
        body.fall = fall;
        self.life.orbs.push(Orb {
            id,
            value,
            count,
            pos,
            vel,
            on_ground,
            age,
            ticks: 0,
            track: Default::default(),
            removed: false,
            health,
            body,
            fire,
            fire_flag: false,
            rank: crate::walk::next_rank(),
            common,
        });
        if let Some(n) = nbt {
            self.load_portal_cooldown(id, n);
        }
        self.orb_joined();
        id
    }

    /// The current player's client forgets every item on the ground and every orb (announced to
    /// every client wherever they are).
    pub(crate) fn client_forgets_items_and_orbs(&self) {
        let ground = self.life.drops.iter().filter(|d| !d.removed).map(|d| d.id);
        let orbs = self.life.orbs.iter().filter(|o| !o.removed).map(|o| o.id);
        let ids: Vec<i32> = ground.chain(orbs).collect();
        if !ids.is_empty() {
            let _ = self.send_to_current(Event::Entity(EntityEvent::Remove(ids)));
        }
    }

    /// The two packets that announce an orb to a client: the entity, then its value.
    fn orb_announcement(&self, id: i32, uuid: u128, pos: [f64; 3], velocity: [f64; 3], value: i32) -> [Event; 2] {
        let add = AddEntity {
            id,
            uuid,
            kind: self.entity_kind("minecraft:experience_orb"),
            pos,
            velocity,
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: value,
        };
        let data = EntityEvent::Data { id, values: vec![(8, Meta::Int(value))] };
        [Event::Entity(EntityEvent::Add(Box::new(add))), Event::Entity(data)]
    }

    /// Every experience orb announced again to the current player's client, which cleared its
    /// entities (respawn). Announced at its tracking base and the base left as it is, like the
    /// game: the other players still see the orbs from that base, and the next relative move is
    /// coded from it for every client.
    pub(crate) fn reannounce_orbs(&self) {
        for o in self.life.orbs.iter().filter(|o| !o.removed) {
            let (pos, vel) = o.track.announce_state(o.pos, o.vel);
            for e in self.orb_announcement(o.id, o.id as u128, pos, vel, o.value) {
                let _ = self.send_to_current(e);
            }
        }
    }

    /// The game's compound of a ground item (common entity part, then its own fields).
    pub(crate) fn item_ent_nbt(&self, d: &ItemEnt) -> Nbt {
        let mut f = vec![("id".to_owned(), Nbt::String("minecraft:item".into()))];
        let cooldown = self.portal_cooldown(d.id);
        let (pos, vel, yaw, fall, fire, on_ground, common) = (d.pos, d.vel, d.yaw, d.body.fall, d.fire, d.on_ground, d.common);
        write_common(&mut f, Part { pos, vel, yaw, fall, fire, on_ground, common, cooldown });
        f.push(("UUID".into(), crate::nbtio::uuid_nbt(ground_uuid(d.id))));
        f.push(("Health".into(), Nbt::Short(d.health as i16)));
        f.push(("Age".into(), Nbt::Short(d.age as i16)));
        f.push(("PickupDelay".into(), Nbt::Short(d.pickup_delay as i16)));
        if d.thrower == Some(crate::mobs::mob::Ent::Player) {
            f.push(("Thrower".into(), crate::nbtio::uuid_nbt(self.uuid))); // W8c
        }
        f.push(("Item".into(), d.stack.to_nbt(None)));
        Nbt::Compound(f)
    }

    /// The game's compound of an experience orb.
    pub(crate) fn orb_ent_nbt(&self, o: &Orb) -> Nbt {
        let mut f = vec![("id".to_owned(), Nbt::String("minecraft:experience_orb".into()))];
        let cooldown = self.portal_cooldown(o.id);
        let (pos, vel, fall, fire, on_ground, common) = (o.pos, o.vel, o.body.fall, o.fire, o.on_ground, o.common);
        write_common(&mut f, Part { pos, vel, yaw: 0.0, fall, fire, on_ground, common, cooldown });
        f.push(("UUID".into(), crate::nbtio::uuid_nbt(ground_uuid(o.id))));
        f.push(("Health".into(), Nbt::Short(o.health as i16)));
        f.push(("Age".into(), Nbt::Short(o.age as i16)));
        f.push(("Value".into(), Nbt::Short(o.value as i16)));
        f.push(("Count".into(), Nbt::Int(o.count)));
        Nbt::Compound(f)
    }

    /// Fields of the ground item at `k` read from `n` like the game's load (summon, chunk,
    /// `data`): velocity, common entity part, health, age, pickup delay; the portal delay is
    /// restored under its identifier.
    pub(crate) fn item_read_nbt(&mut self, k: usize, n: &Nbt) {
        let Some(d) = self.life.drops.get_mut(k) else { return };
        let (fire, on_ground, fall, common) = read_common(n);
        d.vel = crate::nbtio::motion(n.get("Motion")).unwrap_or_default();
        (d.fire, d.on_ground, d.body.fall, d.common) = (fire, on_ground, fall, common);
        d.health = int(n, "Health").unwrap_or(5) as i16 as i32;
        d.age = int(n, "Age").unwrap_or(0) as i16 as i32;
        d.pickup_delay = int(n, "PickupDelay").unwrap_or(0) as i16 as i32;
        let id = d.id;
        self.load_portal_cooldown(id, n);
    }

    /// Fields of the orb at `k` read back from `n` (`data`): health and the common entity part
    /// (value, count and age stay: their reading at summon time is `summon_orb`'s).
    pub(crate) fn orb_read_nbt(&mut self, k: usize, n: &Nbt) {
        let Some(o) = self.life.orbs.get_mut(k) else { return };
        let (fire, on_ground, fall, common) = read_common(n);
        (o.fire, o.on_ground, o.body.fall, o.common) = (fire, on_ground, fall, common);
        if let Some(v) = int(n, "Health") {
            o.health = v as i32;
        }
        let id = o.id;
        self.load_portal_cooldown(id, n);
    }

    /// G5-F05: "on fire" flag (data 0, bit 0x01) of an item or orb, sent when it changes.
    fn fire_flag(&self, id: i32, fire: i32, sent: &mut bool) {
        let on = fire > 0;
        if on != *sent {
            *sent = on;
            let _ = self.send(Event::Entity(EntityEvent::Data { id, values: vec![(0, Meta::Byte(i8::from(on)))] }));
        }
    }

    /// G5-F05: experience spent on repairs when an orb is picked up (`repair_with_xp` effect,
    /// Mending): an equipped and damaged stack is drawn among the slots of the enchantments that
    /// carry the effect and fit the slot (a stack counts once per such enchantment), repaired by
    /// the effect's value; what the repair did not use goes on to another draw. Returns the
    /// experience left for the player.
    pub(crate) fn repair_with_xp(&mut self, mut amount: i32) -> i32 {
        const KEY: &str = "minecraft:repair_with_xp";
        loop {
            // Equipment slots in the game's order: main hand, off hand, feet, legs, chest, head.
            let order = [(36 + self.held as usize, "mainhand"), (45, "offhand"), (8, "feet"), (7, "legs"), (6, "chest"), (5, "head")];
            let mut found: Vec<usize> = Vec::new();
            for (slot, name) in order {
                let Some(s) = self.slots[slot].as_ref() else { continue };
                if !self.life.items.damageable(s) || self.life.items.damage(s) <= 0 {
                    continue;
                }
                for (_, def) in self.ench_defs(s) {
                    let fits = def["slots"].as_array().is_some_and(|g| {
                        g.iter().any(|g| g.as_str().is_some_and(|g| crate::projectiles::ench::slot_in_group(g, name)))
                    });
                    if fits && def["effects"].get(KEY).is_some() {
                        found.push(slot);
                    }
                }
            }
            if found.is_empty() {
                return amount;
            }
            let slot = found[self.life.rng.next_int_bound(found.len() as i32) as usize];
            let Some(mut s) = self.slots[slot].clone() else { return 0 };
            let ctx = crate::projectiles::ench::Ctx { tool: Some(&s), ..Default::default() };
            let to_repair = (self.ench_value(&s, KEY, amount as f32, &ctx) as i32).max(0);
            let damage = self.life.items.damage(&s);
            let repair = to_repair.min(damage);
            // Written even at 0 (an item without a default wear must keep its `damage` to stay usable).
            crate::items::set_component(&mut s, "minecraft:damage", rc_proto::nbt::Nbt::Int(damage - repair));
            self.slots[slot] = Some(s);
            if repair <= 0 {
                return 0;
            }
            // 32-bit products that wrap like the game's (huge custom durability).
            let left = amount.wrapping_sub(repair.wrapping_mul(amount) / to_repair);
            if left <= 0 {
                return 0;
            }
            amount = left;
        }
    }

    /// Players an orb can follow, put in `out`: every player standing in this world in the order
    /// they entered it (the current player first), like the game's search for the nearest one.
    pub(crate) fn orb_followers(&self, out: &mut Vec<Follow>) {
        out.clear();
        out.push(Follow {
            id: self.player_id,
            pos: self.pos,
            eye: self.player_eye_h(),
            spectator: self.game_mode == 3 || !self.player_ready(),
            dead: self.life.vit.dead,
        });
        for c in self.lan_others_here() {
            out.push(Follow {
                id: c.player_id,
                pos: c.pos,
                eye: crate::mobs::guest_dims(c).2,
                spectator: c.game_mode == 3 || !c.world_loaded(self.tick),
                dead: c.vit.dead,
            });
        }
    }

    /// Items and orbs as their own walk (block probes, parked worlds): physics, merge, pickup,
    /// disappearance.
    #[cfg(test)]
    pub(crate) fn drops_tick(&mut self) {
        crate::server::phase("items");
        use crate::walk::{Fam, bit};
        self.walk_alone(bit(Fam::Item) | bit(Fam::Orb), Self::drops_track, Self::drops_post);
    }

    /// Network tracking of the items and orbs BEFORE the walk, like the official server (the
    /// client receives the state of the previous tick then does the current tick itself).
    pub(crate) fn drops_track(&mut self) {
        let mut drops = std::mem::take(&mut self.life.drops);
        for d in drops.iter_mut() {
            d.ticked = false;
            if !d.removed {
                self.track_object(&mut d.track, d.id, d.pos, d.vel, d.on_ground, 20);
            }
        }
        drops.append(&mut self.life.drops);
        self.life.drops = drops;
        let mut orbs = std::mem::take(&mut self.life.orbs);
        for o in orbs.iter_mut().filter(|o| !o.removed) {
            self.track_object(&mut o.track, o.id, o.pos, o.vel, o.on_ground, 20);
        }
        orbs.append(&mut self.life.orbs);
        self.life.orbs = orbs;
    }

    /// Items of the slots from `start` whose rank stays under `limit` (before `end`), in the
    /// entity walk; returns the slot after them. A run of at least an eighth of the list ticks
    /// with the list taken out once, the plates reading every item's position from
    /// `contact.live_items` (O(list) once per run); a shorter one (ranks interleaved with other
    /// families) ticks each item out of its slot (`item_visit`). Both are O(1) per item.
    pub(crate) fn item_run(&mut self, start: usize, end: usize, limit: u64) -> usize {
        let mut stop = start + 1;
        while stop < end && self.life.drops.get(stop).is_some_and(|d| d.rank < limit) {
            stop += 1;
        }
        if one_by_one(start, stop, self.life.drops.len()) || self.contact.items_taken {
            for i in start..stop {
                self.item_visit(i);
            }
            return stop;
        }
        let mut drops = std::mem::take(&mut self.life.drops);
        let mut live = std::mem::take(&mut self.contact.live_items);
        live.clear();
        live.extend(drops.iter().map(|d| (d.pos, d.removed)));
        self.contact.live_items = live;
        self.contact.items_taken = true;
        self.walk.item_grid.reset();
        for i in start..stop.min(drops.len()) {
            let (left, rest) = drops.split_at_mut(i);
            let (d, right) = rest.split_first_mut().expect("slot of the run");
            if self.item_meets(d) {
                self.item_tick(d, i, Some([left, right]));
            }
        }
        self.walk.item_grid.reset();
        self.contact.items_taken = false;
        self.contact.live_items.clear();
        drops.append(&mut self.life.drops);
        self.life.drops = drops;
        stop
    }

    /// Item of slot `i`, at its rank in the entity walk. Ticked out of its slot (a removed
    /// placeholder holds it): the other items stay in the list, the plates count it through
    /// `contact.ticking_item`.
    pub(crate) fn item_visit(&mut self, i: usize) {
        let Some(d) = self.life.drops.get(i).filter(|d| !d.removed) else { return };
        // G3-F13: outside a "ticking entities" chunk, no tick (and no merge into it) this tick.
        let (ticking, mut rank) = (self.entity_ticks_at(d.pos), d.rank);
        let meet = self.meet(&mut rank, ticking, crate::walk::Fam::Item);
        self.life.drops[i].rank = rank;
        if meet != crate::walk::Meet::Tick {
            return;
        }
        let spare = self.walk.spare_item.take().unwrap_or_else(ItemEnt::placeholder);
        let mut d = std::mem::replace(&mut self.life.drops[i], spare);
        d.ticked = true;
        self.contact.ticking_item = Some((d.pos, false));
        self.item_tick(&mut d, i, None);
        self.contact.ticking_item = None;
        if let Some(slot) = self.life.drops.get_mut(i) {
            self.walk.spare_item = Some(std::mem::replace(slot, d));
        }
    }

    /// The item of slot `i` met by the walk ticks now (G3-F13: not outside a "ticking entities"
    /// chunk).
    fn item_meets(&mut self, d: &mut ItemEnt) -> bool {
        if d.removed {
            return false;
        }
        let ticking = self.entity_ticks_at(d.pos);
        d.ticked = self.meet(&mut d.rank, ticking, crate::walk::Fam::Item) == crate::walk::Meet::Tick;
        d.ticked
    }

    /// Position and removal of the item of slot `i` being ticked, as the plates see them.
    fn item_seen(&mut self, i: usize, pos: [f64; 3], gone: bool) {
        if self.contact.items_taken {
            if let Some(l) = self.contact.live_items.get_mut(i) {
                *l = (pos, gone);
            }
            self.walk.item_grid.moved(i, pos, gone);
        } else {
            self.contact.ticking_item = Some((pos, gone));
        }
    }

    /// Tick of the item of slot `i`, out of its list: `taken`, the slots before and after it
    /// when the list is taken out for a run; `None`, it sits in place (a placeholder in slot `i`).
    fn item_tick(&mut self, d: &mut ItemEnt, i: usize, taken: Option<[&mut [ItemEnt]; 2]>) {
        {
            d.ticks += 1;
            d.common.count_down();
            // G5-F05: fire at the start of its tick (an immune stack is put out).
            let immune = d.fire > 0 && self.item_resists(&d.stack, "minecraft:in_fire");
            d.body.pos = d.pos;
            let lava = burn_due(d.fire, immune) && d.body.touching_lava(&self.world(), &self.phys);
            if burn_step(&mut d.fire, immune, lava) {
                self.item_hurt_ent(d, "minecraft:on_fire", 1.0, false);
            }
            self.fire_flag(d.id, d.fire, &mut d.fire_flag);
            if d.removed {
                self.item_seen(i, d.pos, true);
                return;
            }
            if d.pickup_delay > 0 && d.pickup_delay != 32767 {
                d.pickup_delay -= 1;
            }
            let old = d.pos;
            // Same physics as the client (`rc_physics::body`): single-precision constants,
            // braking block, water and lava, currents, block effects, block push-out. Its free-room
            // test and its movement meet the solid entities, as the client's do.
            let (mv, wet, below, support) = {
                let w = self.world_for_room(Self::room_for_object(d.id, d.pos));
                let b = &mut d.body;
                // Water state at the start of the tick (the one the movement reads).
                let wet = b.was_in_water();
                (b.pos, b.vel, b.on_ground) = (d.pos, d.vel, d.on_ground);
                b.fast_lava = self.fast_lava();
                let mut r = crate::mobs::jrand::JRand::new(i64::from(d.id) << 32 ^ i64::from(d.ticks));
                b.tick_item_server(&w, &self.phys, d.ticks, d.id, &mut || r.next_float());
                (d.pos, d.vel, d.on_ground) = (b.pos, b.vel, b.on_ground);
                (b.moved, wet, b.below(&self.phys, &w, 0.2), b.support())
            };
            // Fall distance then landing (`hit_ground` event, block effects).
            if self.fall_step(&mut d.body.fall, old, mv, wet, d.on_ground) {
                let dampens = dampens_vibrations(&d.stack.id);
                self.landing_effects(d.pos, below, support, Src::Item { owner: None, dampens });
            }
            if d.on_ground {
                d.body.fall = 0.0;
            }
            // The block it rests on (step effect), before the blocks it is in, like the game: a
            // shrieker answers its thrower.
            if let Some(under) = d.body.stepped_on {
                let src = || Src::Item { owner: d.thrower, dampens: dampens_vibrations(&d.stack.id) };
                self.object_step_on(under, d.pos, src);
            }
            // Blocks passed through (or, at rest, those it rests in): a hopper takes it at once.
            self.hopper_item_inside(old, d);
            if !d.removed {
                // G5-F05: fire, lava, cactus, water and powder snow of the blocks crossed.
                self.inside_blocks(&mut crate::inside_blocks::Who::Item(d));
            }
            self.item_seen(i, d.pos, d.removed);
            if d.removed {
                return;
            }
            // G5-F02e: plates and tripwire right after its movement (the following ones not moved yet).
            self.contact_triggers(old, d.pos, ITEM_SIZE / 2.0, ITEM_SIZE);
            // Merge with its neighbours in its own tick, before its age moves (every 2 ticks
            // when it changed block, otherwise 40).
            d.moved = (0..3).any(|a| old[a].floor() != d.pos[a].floor());
            if d.ticks % if d.moved { 2 } else { 40 } == 0 {
                self.item_merge(d, i, taken);
                if d.removed {
                    self.item_seen(i, d.pos, true);
                    return;
                }
            }
            if d.age != -32768 {
                d.age += 1;
            }
            if d.age >= LIFETIME || d.pos[1] < f64::from(self.dim.height.min_y) - 64.0 {
                d.removed = true;
                self.item_seen(i, d.pos, true);
            }
        }
    }

    /// Merge of the ticking item `d` (slot `i`) with the items around it, in list order, then
    /// those born during this walk: the biggest stack absorbs the smallest (`d` absorbed at
    /// equal counts), stops once `d` is gone. `taken`: see `item_tick`; the items it removes are
    /// marked in `contact.live_items` for the plates.
    fn item_merge(&mut self, d: &mut ItemEnt, i: usize, taken: Option<[&mut [ItemEnt]; 2]>) {
        let switches = DropSwitches::current();
        if !switches.merge_early_exit {
            return self.item_merge_every_pair(d, i, taken);
        }
        // These never change while `d` merges: an item that cannot merge now never will this tick.
        let max = self.life.items.max_stack(&d.stack);
        if !may_merge(d, max) {
            return;
        }
        let reach = merge_reach(d.pos);
        // In place: the whole list (placeholder in slot `i`); run: the items born since it began.
        let mut rest = std::mem::take(&mut self.life.drops);
        if let Some(run) = taken {
            self.item_merge_in_run(d, i, run, &reach, max, switches);
        }
        if !d.removed {
            for o in &mut rest {
                self.item_merge_with(d, o, &reach, max);
                if d.removed {
                    break;
                }
            }
        }
        rest.append(&mut self.life.drops);
        self.life.drops = rest;
    }

    /// The part of `item_merge` over the list of a run (`d` taken from slot `i`, the other
    /// slots in `run`): candidates from the grid of a long run, otherwise every slot, the
    /// distance tested on the compact live positions first.
    fn item_merge_in_run(
        &mut self,
        d: &mut ItemEnt,
        i: usize,
        [left, right]: [&mut [ItemEnt]; 2],
        reach: &Aabb,
        max: i32,
        switches: DropSwitches,
    ) {
        let len = left.len() + 1 + right.len();
        if switches.merge_grid && len >= ITEM_GRID_FROM {
            let mut grid = std::mem::take(&mut self.walk.item_grid);
            if !grid.built {
                grid.build(&self.contact.live_items);
            }
            let found = grid.near(reach, &self.contact.live_items);
            for k in 0..found {
                let j = grid.candidates[k] as usize;
                if j != i && self.item_merge_with(d, run_slot(left, right, i, j), reach, max) {
                    self.item_gone_in_run(j);
                    grid.moved(j, [0.0; 3], true);
                }
                if d.removed {
                    break;
                }
            }
            self.walk.item_grid = grid;
            return;
        }
        for j in (0..len).filter(|&j| j != i) {
            // The live position of a slot is that of its item (only the ticked one moves in a run).
            let far = switches.merge_live_prefilter
                && self.contact.live_items.get(j).is_some_and(|l| !crosses(&bbox(l.0, ITEM_SIZE), reach));
            if !far && self.item_merge_with(d, run_slot(left, right, i, j), reach, max) {
                self.item_gone_in_run(j);
            }
            if d.removed {
                break;
            }
        }
    }

    /// The item of slot `j` of the run in progress was absorbed whole: the plates no longer see it.
    fn item_gone_in_run(&mut self, j: usize) {
        if let Some(l) = self.contact.live_items.get_mut(j) {
            l.1 = true;
        }
    }

    /// One merge between the ticking item `d` (mergeable, stack size `max`, merge reach `reach`)
    /// and `o`; true when `o` was absorbed whole. Cheapest tests first: removal, then distance.
    fn item_merge_with(&mut self, d: &mut ItemEnt, o: &mut ItemEnt, reach: &Aabb, max: i32) -> bool {
        count_merge_pair();
        if o.removed
            || !crosses(&bbox(o.pos, ITEM_SIZE), reach)
            || !may_merge(o, max)
            || d.stack.count >= max
            || d.stack.count + o.stack.count > max
            || !same(&d.stack, &o.stack)
        {
            return false;
        }
        self.merge_stacks(d, o, max)
    }

    /// `item_merge` before `RUSTCRAFT_OPT_ITEM_MERGE_EARLY_EXIT`: every pair tested in full.
    fn item_merge_every_pair(&mut self, d: &mut ItemEnt, i: usize, taken: Option<[&mut [ItemEnt]; 2]>) {
        let mut rest = std::mem::take(&mut self.life.drops);
        if let Some([left, right]) = taken {
            let slots = left.iter_mut().enumerate().chain(right.iter_mut().enumerate().map(|(k, o)| (i + 1 + k, o)));
            for (j, o) in slots {
                if self.item_merge_pair(d, o) {
                    self.item_gone_in_run(j);
                }
                if d.removed {
                    break;
                }
            }
        }
        if !d.removed {
            for o in &mut rest {
                self.item_merge_pair(d, o);
                if d.removed {
                    break;
                }
            }
        }
        rest.append(&mut self.life.drops);
        self.life.drops = rest;
    }

    /// One merge between the ticking item `d` and `o`, every test redone; true when `o` was
    /// absorbed whole.
    fn item_merge_pair(&mut self, d: &mut ItemEnt, o: &mut ItemEnt) -> bool {
        count_merge_pair();
        if o.removed || d.removed {
            return false;
        }
        let max = self.life.items.max_stack(&d.stack);
        if !crosses(&bbox(o.pos, ITEM_SIZE), &merge_reach(d.pos))
            || !may_merge(d, max)
            || !may_merge(o, max)
            || !same(&d.stack, &o.stack)
            || d.stack.count + o.stack.count > max
        {
            return false;
        }
        self.merge_stacks(d, o, max)
    }

    /// The biggest stack absorbs the smallest (`d` absorbed at equal counts), up to `max` and
    /// 64; true when `o` was absorbed whole.
    fn merge_stacks(&self, d: &mut ItemEnt, o: &mut ItemEnt, max: i32) -> bool {
        let (to, from) = if o.stack.count < d.stack.count { (&mut *d, &mut *o) } else { (&mut *o, &mut *d) };
        let moved = from.stack.count.min(max.min(64) - to.stack.count);
        to.stack.count += moved;
        from.stack.count -= moved;
        to.pickup_delay = to.pickup_delay.max(from.pickup_delay);
        to.age = to.age.min(from.age);
        let net = self.net(Some(&to.stack));
        let _ = self.send(Event::Entity(EntityEvent::Data { id: to.id, values: vec![(8, Meta::Item(net))] }));
        if from.stack.count <= 0 {
            from.removed = true;
        }
        o.removed
    }

    /// After the walk: pickup by the installed player (its own tick, after the world: the host,
    /// or the first guest of a world ticked away from the host's), then removals. The other LAN
    /// guests pick up in their own tick (`lan::guest_tick`). A player hidden from the world
    /// (absent, or gone during its tick) is not ready and picks up nothing.
    pub(crate) fn drops_post(&mut self) {
        self.pick_up_touched_drops();
        self.remove_gone_drops();
    }

    /// The current player's pickups, like the game's player tick: every item its box inflated by
    /// 1 / 0.5 / 1 touches, then one touched orb drawn at random. Nothing for a dead player, a
    /// spectator or a player whose client has not loaded the world yet.
    pub(crate) fn pick_up_touched_drops(&mut self) {
        if self.life.vit.dead || self.game_mode == 3 || !self.player_ready() {
            return;
        }
        let mut drops = std::mem::take(&mut self.life.drops);
        self.pick_up_touched_items(&mut drops);
        // Items appeared during the pickup (contents of a bag…): after the others.
        drops.append(&mut self.life.drops);
        self.life.drops = drops;
        let mut orbs = std::mem::take(&mut self.life.orbs);
        self.orbs_pickup(&mut orbs);
        orbs.append(&mut self.life.orbs);
        self.life.orbs = orbs;
    }

    /// Items of `drops` the current player touches go to its inventory (what does not fit stays).
    fn pick_up_touched_items(&mut self, drops: &mut [ItemEnt]) {
        let h = if self.sneaking { 1.5 } else { 1.8 };
        let reach = Aabb {
            min: [self.pos[0] - 1.3, self.pos[1] - 0.5, self.pos[2] - 1.3],
            max: [self.pos[0] + 1.3, self.pos[1] + h + 0.5, self.pos[2] + 1.3],
        };
        for d in drops.iter_mut().filter(|d| !d.removed && d.pickup_delay == 0) {
            let b = bbox(d.pos, ITEM_SIZE);
            if !(0..3).all(|a| b.max[a] > reach.min[a] && b.min[a] < reach.max[a]) {
                continue;
            }
            let before = d.stack.count;
            let creative = self.game_mode == 1;
            let selected = self.held as usize;
            let rest = self.pickup_into_inventory(d.stack.clone(), creative, selected);
            if crate::life::s2log() {
                let where_: Vec<(usize, String)> =
                    self.slots.iter().enumerate().filter_map(|(i, s)| Some((i, s.as_ref()?.id.clone()))).collect();
                eprintln!("singleplayer: pickup {} (held {selected}) → {where_:?}", d.stack.id);
            }
            let after = rest.as_ref().map_or(0, |s| s.count);
            if after < before {
                self.item_picked_up(d, before, rest);
            }
        }
    }

    /// The current player took (part of) `d`, which held `before` items: progress, the pickup
    /// packet, and the item removed or shrunk to `rest`.
    fn item_picked_up(&mut self, d: &mut ItemEnt, before: i32, rest: Option<Stack>) {
        self.s9_picked_up(&d.stack.id, before); // S9
        // W8c: the reaction to the pickup of a player item: stack seen whole if everything is taken,
        // otherwise what remains; player thrower → `…_by_entity` (the player), any thrower →
        // `…_by_player`.
        let seen = rest.clone().unwrap_or_else(|| d.stack.clone());
        if let Some(t) = d.thrower {
            use crate::progress::pred::Who;
            let me = Who::E(crate::commands::ents::E::Player);
            if t == crate::mobs::mob::Ent::Player {
                self.trig_picked_up("minecraft:thrown_item_picked_up_by_entity", &seen, Some(me));
            }
            let mob = match t {
                crate::mobs::mob::Ent::Mob(id) => self.mob_ref(id).map(|m| (*m).clone()),
                crate::mobs::mob::Ent::Player => None,
            };
            let who = match &mob {
                Some(m) => Some(Who::Mob(m)),
                None if t == crate::mobs::mob::Ent::Player => Some(me),
                None => None,
            };
            if who.is_some() {
                self.trig_picked_up("minecraft:thrown_item_picked_up_by_player", &seen, who);
            }
        }
        let _ =
            self.send(Event::Misc(rc_client::misc::Misc::Take { item: d.id, collector: self.player_id, amount: before }));
        match rest {
            None => d.removed = true,
            Some(r) => {
                d.stack = r;
                let net = self.net(Some(&d.stack));
                let _ = self.send(Event::Entity(EntityEvent::Data { id: d.id, values: vec![(8, Meta::Item(net))] }));
            }
        }
    }

    /// Items and orbs removed (picked up, burnt, merged, out of the world) leave their lists, and
    /// the clients are told in one packet.
    pub(crate) fn remove_gone_drops(&mut self) {
        let mut drops = std::mem::take(&mut self.life.drops);
        let gone: Vec<i32> = drops.iter().filter(|d| d.removed).map(|d| d.id).collect();
        let pool = DropSwitches::current().body_pool;
        if pool {
            for d in drops.extract_if(.., |d| d.removed) {
                self.keep_body(d.body);
            }
        } else {
            drops.retain(|d| !d.removed);
        }
        drops.append(&mut self.life.drops);
        self.life.drops = drops;
        let mut orbs = std::mem::take(&mut self.life.orbs);
        let gone_orbs: Vec<i32> = orbs.iter().filter(|o| o.removed).map(|o| o.id).collect();
        if pool {
            for o in orbs.extract_if(.., |o| o.removed) {
                self.keep_body(o.body);
            }
        } else {
            orbs.retain(|o| !o.removed);
        }
        orbs.append(&mut self.life.orbs);
        self.life.orbs = orbs;
        let all: Vec<i32> = gone.into_iter().chain(gone_orbs).collect();
        if !all.is_empty() {
            // Scoreboard: items and orbs removed (command UUID derived from the id).
            if self.cmd.scores.has_entries() {
                for &i in &all {
                    self.entity_scores_removed(self.ent_uuid(crate::commands::ents::E::Item(i)));
                }
            }
            let _ = self.send(Event::Entity(EntityEvent::Remove(all)));
        }
    }

    /// Storing a picked-up item: returns what did not fit.
    pub(crate) fn pickup_into_inventory(&mut self, s: Stack, creative: bool, selected: usize) -> Option<Stack> {
        if DropSwitches::current().pickup_in_place {
            return self.pickup_in_place(s, creative, selected);
        }
        let items = &self.life.items;
        let count = s.count;
        // Adding an item to the inventory completes as best it can; the rest stays on the ground.
        let mut probe = self.slots.clone();
        match inventory_add(&mut probe, items, selected, s.clone(), creative) {
            Some(rest) if rest.count == count => Some(rest),
            _ => {
                // Starts again counting what was placed.
                let before: i32 = self.slots.iter().flatten().filter(|t| same(t, &s)).map(|t| t.count).sum();
                let _ = inventory_add(&mut self.slots, items, selected, s.clone(), creative);
                let after: i32 = self.slots.iter().flatten().filter(|t| same(t, &s)).map(|t| t.count).sum();
                let placed = if creative { count } else { after - before };
                (placed < count).then(|| Stack { count: count - placed, ..s })
            }
        }
    }

    /// [`Server::pickup_into_inventory`] without the trial on a copy of the slots: adding to the
    /// inventory touches no slot when nothing fits (its first step always stores at least one
    /// item when it finds a slot), so the real slots answer the same question.
    fn pickup_in_place(&mut self, s: Stack, creative: bool, selected: usize) -> Option<Stack> {
        let count = s.count;
        let held =
            |slots: &[Option<Stack>]| -> i32 { slots.iter().flatten().filter(|t| same(t, &s)).map(|t| t.count).sum() };
        let before = held(&self.slots);
        match inventory_add(&mut self.slots, &self.life.items, selected, s.clone(), creative) {
            Some(rest) if rest.count == count => Some(rest),
            _ => {
                let placed = if creative { count } else { held(&self.slots) - before };
                (placed < count).then(|| Stack { count: count - placed, ..s })
            }
        }
    }

    /// Physics of a new server item or orb: a body kept from a removed one when there is one
    /// (`RUSTCRAFT_OPT_BODY_POOL`), reset to the state of a new one.
    fn new_body(&mut self, pos: [f64; 3], vel: [f64; 3], size: f64, eye: f64) -> Box<Body> {
        let Some(mut b) = self.walk.spare_bodies.pop() else { return server_body(pos, vel, size, eye) };
        *b = Body::new(pos, vel, size, size, eye);
        b.server = true;
        b
    }

    /// The body of a removed item or orb, kept for the next one born (up to `SPARE_BODIES`).
    fn keep_body(&mut self, b: Box<Body>) {
        if self.walk.spare_bodies.len() < SPARE_BODIES {
            self.walk.spare_bodies.push(b);
        }
    }

    /// Orbs of the slots from `start` whose rank stays under `limit` (before `end`), in the entity
    /// walk; returns the slot after them. Same two ways as the items (`item_run`): a long run with
    /// the list taken out once and the positions in `contact.live_orbs`, a short one orb by orb.
    pub(crate) fn orb_run(&mut self, start: usize, end: usize, limit: u64) -> usize {
        let mut stop = start + 1;
        while stop < end && self.life.orbs.get(stop).is_some_and(|o| o.rank < limit) {
            stop += 1;
        }
        if one_by_one(start, stop, self.life.orbs.len()) || self.contact.orbs_taken {
            for i in start..stop {
                self.orb_visit(i);
            }
            return stop;
        }
        let mut orbs = std::mem::take(&mut self.life.orbs);
        let mut live = std::mem::take(&mut self.contact.live_orbs);
        live.clear();
        live.extend(orbs.iter().map(|o| (o.pos, o.removed)));
        self.contact.live_orbs = live;
        self.contact.orbs_taken = true;
        for i in start..stop.min(orbs.len()) {
            if !self.orb_meets(&mut orbs[i]) {
                continue;
            }
            let gone = self.orb_fire(&mut orbs[i], i);
            if orbs[i].ticks % 20 == 1 {
                self.orbs_merge(&mut orbs, i);
            }
            if !gone {
                self.orb_move(&mut orbs[i], i);
            }
        }
        self.contact.orbs_taken = false;
        self.contact.live_orbs.clear();
        orbs.append(&mut self.life.orbs);
        self.life.orbs = orbs;
        stop
    }

    /// Orb of slot `i`, at its rank in the entity walk. Ticked out of its slot like an item, put
    /// back for its merge (which reads the whole list).
    pub(crate) fn orb_visit(&mut self, i: usize) {
        let Some(o) = self.life.orbs.get(i).filter(|o| !o.removed) else { return };
        // G3-F13: outside a "ticking entities" chunk, it waits.
        let (ticking, mut rank) = (self.entity_ticks_at(o.pos), o.rank);
        let meet = self.meet(&mut rank, ticking, crate::walk::Fam::Orb);
        self.life.orbs[i].rank = rank;
        if meet != crate::walk::Meet::Tick {
            return;
        }
        let spare = self.walk.spare_orb.take().unwrap_or_else(Orb::placeholder);
        let mut o = std::mem::replace(&mut self.life.orbs[i], spare);
        let gone = self.orb_fire(&mut o, i);
        if o.ticks % 20 == 1 {
            let spare = std::mem::replace(&mut self.life.orbs[i], o);
            let mut orbs = std::mem::take(&mut self.life.orbs);
            self.orbs_merge(&mut orbs, i);
            orbs.append(&mut self.life.orbs);
            self.life.orbs = orbs;
            o = std::mem::replace(&mut self.life.orbs[i], spare);
        }
        if !gone {
            self.contact.ticking_orb = Some((o.pos, false));
            self.orb_move(&mut o, i);
            self.contact.ticking_orb = None;
        }
        if let Some(slot) = self.life.orbs.get_mut(i) {
            self.walk.spare_orb = Some(std::mem::replace(slot, o));
        }
    }

    /// The orb of slot `i` met by the walk ticks now (G3-F13: not outside a "ticking entities"
    /// chunk).
    fn orb_meets(&mut self, o: &mut Orb) -> bool {
        if o.removed {
            return false;
        }
        let ticking = self.entity_ticks_at(o.pos);
        self.meet(&mut o.rank, ticking, crate::walk::Fam::Orb) == crate::walk::Meet::Tick
    }

    /// Position and removal of the orb of slot `i` being ticked, as the plates see them.
    fn orb_seen(&mut self, i: usize, pos: [f64; 3], gone: bool) {
        if self.contact.orbs_taken {
            if let Some(l) = self.contact.live_orbs.get_mut(i) {
                *l = (pos, gone);
            }
        } else {
            self.contact.ticking_orb = Some((pos, gone));
        }
    }

    /// Start of an orb's tick (G5-F05): fire, below the world. True when it is gone.
    fn orb_fire(&mut self, o: &mut Orb, i: usize) -> bool {
        o.ticks += 1;
        o.common.count_down();
        o.body.pos = o.pos;
        let lava = burn_due(o.fire, false) && o.body.touching_lava(&self.world(), &self.phys);
        if burn_step(&mut o.fire, false, lava) {
            self.orb_hurt_ent(o, "minecraft:on_fire", 1.0);
        }
        self.fire_flag(o.id, o.fire, &mut o.fire_flag);
        if o.pos[1] < f64::from(self.dim.height.min_y) - 64.0 {
            o.removed = true;
        }
        if o.removed {
            self.orb_seen(i, o.pos, true);
        }
        o.removed
    }

    /// G5-F05: orb `i` absorbs the orbs of the same value around it every 20 ticks (from the
    /// first); from 40 orbs on, candidates read from the residues modulo 40 built once per walk
    /// (below, the whole list costs less; `RUSTCRAFT_OPT_ORB_MERGE_LINEAR=1`: always it).
    /// Quirk kept: an orb just removed (burnt, below the world) still absorbs its neighbours,
    /// which disappear with it.
    fn orbs_merge(&mut self, orbs: &mut [Orb], i: usize) {
        let mut live = std::mem::take(&mut self.contact.live_orbs);
        if orbs.len() < ORB_BUCKETS_FROM || rc_data::env_flag!("RUSTCRAFT_OPT_ORB_MERGE_LINEAR") {
            let n = orbs.len();
            merge_orbs(orbs, i, &mut live, 0..n);
        } else {
            let mut buckets = std::mem::take(&mut self.contact.orb_buckets);
            if !self.walk.buckets_built {
                orb_buckets(orbs, &mut buckets);
                self.walk.buckets_built = true;
            }
            let b = &buckets[orbs[i].id.rem_euclid(40) as usize];
            merge_orbs(orbs, i, &mut live, b.iter().map(|&j| j as usize));
            self.contact.orb_buckets = buckets;
        }
        self.contact.live_orbs = live;
    }

    /// Movement of a living orb: same physics as the client (`rc_physics::body`), no skipped
    /// tick; landing, crossed blocks, plates, age.
    fn orb_move(&mut self, o: &mut Orb, i: usize) {
        // The followers' list is rebuilt for each orb (players move during the walk) in a buffer
        // kept by the walk, so that no orb allocates.
        let mut players = std::mem::take(&mut self.walk.orb_followers);
        self.orb_followers(&mut players);
        let from = o.pos;
        let wet = o.body.was_in_water();
        let (mv, below, support) = self.orb_physics(o, &players);
        self.walk.orb_followers = players;
        if self.fall_step(&mut o.body.fall, from, mv, wet, o.on_ground) {
            self.landing_effects(o.pos, below, support, Src::Other);
        }
        if o.on_ground {
            o.body.fall = 0.0;
        }
        // The block it rests on (step effect), before the blocks it is in, like the game.
        if let Some(under) = o.body.stepped_on {
            self.object_step_on(under, o.pos, || Src::Other);
        }
        // G5-F05: fire, lava, cactus, water and powder snow of the blocks crossed.
        self.inside_blocks(&mut crate::inside_blocks::Who::Orb(o));
        if o.removed {
            self.orb_seen(i, o.pos, true);
            return;
        }
        // G5-F02e: plates and tripwire right after its movement.
        self.orb_seen(i, o.pos, false);
        self.contact_triggers(from, o.pos, ORB_SIZE / 2.0, ORB_SIZE);
        o.age += 1;
        if o.age >= LIFETIME {
            o.removed = true;
            self.orb_seen(i, o.pos, true);
        }
    }

    /// One physics step of orb `o`, drawn towards the nearest of `players` within 8 blocks; returns
    /// its movement, the block under it and the block that holds it up.
    fn orb_physics(&self, o: &mut Orb, players: &[Follow]) -> ([f64; 3], [i32; 3], Option<[i32; 3]>) {
        // Its "colliding" tests and its movement meet the solid entities, as the client's do; the
        // tests, made for no entity, skip the border (`Body::tick_orb_server`).
        let w = self.world_for_room(Self::room_for_object(o.id, o.pos));
        let b = &mut o.body;
        (b.pos, b.vel, b.on_ground) = (o.pos, o.vel, o.on_ground);
        b.fast_lava = self.fast_lava();
        let mut r = crate::mobs::jrand::JRand::new(i64::from(o.id) << 32 ^ i64::from(o.ticks));
        b.tick_orb_server(&w, &self.phys, players, &mut || r.next_float());
        (o.pos, o.vel, o.on_ground) = (b.pos, b.vel, b.on_ground);
        (b.moved, b.below(&self.phys, &w, 0.2), b.support())
    }

    /// Orb pickup by the current player (alive, not a spectator, world loaded): one orb every 2
    /// ticks.
    fn orbs_pickup(&mut self, orbs: &mut [Orb]) {
        if self.life.vit.xp_delay > 0 {
            self.life.vit.xp_delay -= 1;
        }
        let h = if self.sneaking { 1.5 } else { 1.8 };
        let touching: Vec<usize> = orbs
            .iter()
            .enumerate()
            .filter(|(_, o)| !o.removed)
            .filter(|(_, o)| {
                let b = bbox(o.pos, ORB_SIZE);
                b.max[0] > self.pos[0] - 1.3
                    && b.min[0] < self.pos[0] + 1.3
                    && b.max[1] > self.pos[1] - 0.5
                    && b.min[1] < self.pos[1] + h + 0.5
                    && b.max[2] > self.pos[2] - 1.3
                    && b.min[2] < self.pos[2] + 1.3
            })
            .map(|(i, _)| i)
            .collect();
        if !touching.is_empty() && self.life.vit.xp_delay == 0 {
            let i = touching[self.life.rng.next_int_bound(touching.len() as i32) as usize];
            self.life.vit.xp_delay = 2;
            let _ = self.send(Event::Misc(rc_client::misc::Misc::Take {
                item: orbs[i].id,
                collector: self.player_id,
                amount: 1,
            }));
            // G5-F05: Mending repairs first, the player gets what is left. The pickup sound
            // is the client's own, played when it reads the pickup packet.
            let left = self.repair_with_xp(orbs[i].value);
            if left > 0 {
                self.give_xp(left);
            }
            orbs[i].count -= 1;
            if orbs[i].count <= 0 {
                orbs[i].removed = true;
            }
        }
    }

    /// Ground items, experience orbs, primed TNT and falling blocks read with a chunk: made live
    /// (pickable, simulated).
    pub(crate) fn adopt_items(&mut self, loaded: &[(i32, i32)]) {
        use rc_proto::nbt::Nbt;
        const KINDS: [&str; 4] =
            ["minecraft:item", "minecraft:experience_orb", "minecraft:tnt", "minecraft:falling_block"];
        for p in loaded {
            let Some(list) = self.entities.get_mut(p) else { continue };
            type Ents = Vec<(i32, Nbt)>;
            let (items, rest): (Ents, Ents) = std::mem::take(list)
                .into_iter()
                .partition(|(_, e)| e.get("id").and_then(Nbt::as_str).is_some_and(|k| KINDS.contains(&k)));
            *list = rest;
            if items.is_empty() {
                continue;
            }
            self.life.drop_chunks.insert(*p);
            for (_, e) in items {
                // G5-F05: orb (value, count, age, health, fire, speed as the game reads them).
                let id = e.get("id").and_then(Nbt::as_str).unwrap_or_default();
                if id == KINDS[1] {
                    let pos = crate::nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
                    self.summon_orb(pos, Some(&e));
                    continue;
                }
                if id == KINDS[2] || id == KINDS[3] {
                    let pos = crate::nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
                    let id = id.to_owned();
                    self.summon_sim_entity(&id, pos, Some(&e));
                    continue;
                }
                let Some(stack) = e.get("Item").and_then(Stack::from_nbt) else { continue };
                let pos = crate::nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
                let vel = crate::nbtio::motion(e.get("Motion")).unwrap_or_default();
                let delay = crate::nbtio::int(&e, "PickupDelay").unwrap_or(0) as i32;
                // W8c: thrower re-read (only the player is recognized).
                let thrower = crate::nbtio::uuid(e.get("Thrower")) == Some(self.uuid);
                let before = self.life.drops.len();
                self.spawn_item(pos, vel, stack, delay);
                if self.life.drops.len() > before {
                    self.item_read_nbt(before, &e);
                    let d = &mut self.life.drops[before];
                    if let Some([yaw, _]) = crate::nbtio::f32s::<2>(e.get("Rotation")) {
                        d.yaw = yaw;
                    }
                    if thrower {
                        d.thrower = Some(crate::mobs::mob::Ent::Player);
                    }
                }
            }
        }
    }

    /// Entity chunks to rewrite (`entities/`): those that carry ground items, or
    /// carried them at the previous save; entities read kept as they are.
    pub(crate) fn drops_save_jobs(&mut self, jobs: &mut Vec<crate::save::Job>) {
        use rc_proto::nbt::Nbt;
        use std::collections::BTreeMap;
        let mut by_chunk: BTreeMap<(i32, i32), Vec<Nbt>> = BTreeMap::new();
        // L10-ENT: the same compound as `/data` shows, with the game's common entity part
        // (fall distance, air, invulnerability, portal delay included).
        for d in &self.life.drops {
            let c = ((d.pos[0].floor() as i32) >> 4, (d.pos[2].floor() as i32) >> 4);
            by_chunk.entry(c).or_default().push(self.item_ent_nbt(d));
        }
        // G5-F05: experience orbs, written like the game (never saved before: lost on reload).
        for o in self.life.orbs.iter().filter(|o| !o.removed) {
            let c = ((o.pos[0].floor() as i32) >> 4, (o.pos[2].floor() as i32) >> 4);
            by_chunk.entry(c).or_default().push(self.orb_ent_nbt(o));
        }
        self.sim_entities_into_chunks(&mut by_chunk); // G4-F08: primed TNT, falling blocks
        self.mobs_into_chunks(&mut by_chunk); // S4: mobs
        self.projectiles_into_chunks(&mut by_chunk); // S8: projectiles, clouds
        self.carts_into_chunks(&mut by_chunk); // G5-F06: minecarts
        let now: std::collections::BTreeSet<(i32, i32)> = by_chunk.keys().copied().collect();
        let mut all = now.clone();
        all.extend(self.life.drop_chunks.iter().copied());
        for c in all {
            let fresh = by_chunk.remove(&c).unwrap_or_default();
            let nbt = self.entity_chunk_bytes(c, fresh);
            let path = self.dim.dir.join("entities").join(crate::region::region_name(c.0, c.1));
            jobs.push(crate::save::Job::Chunk { path, cx: c.0, cz: c.1, nbt });
        }
        self.life.drop_chunks = now;
    }

    /// The entity chunk `c` as saved: the entities kept as they were read (lead knots left out:
    /// the held mob recreates its knot on reload), then `fresh`.
    /// `RUSTCRAFT_OPT_ENTITY_SAVE_LENT`: the kept compounds are lent to the encoding (moved out
    /// of their list, then back) instead of cloned.
    fn entity_chunk_bytes(&mut self, c: (i32, i32), fresh: Vec<Nbt>) -> Vec<u8> {
        let saved = |e: &(i32, Nbt)| e.1.get("id").and_then(Nbt::as_str) != Some("minecraft:leash_knot");
        let root = |ents: Vec<Nbt>| {
            Nbt::Compound(vec![
                ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)),
                ("Position".into(), Nbt::IntArray(vec![c.0, c.1])),
                ("Entities".into(), Nbt::List(ents)),
            ])
        };
        let lend = DropSwitches::current().save_lent;
        let Some(list) = self.entities.get_mut(&c).filter(|_| lend) else {
            let kept = self.entities.get(&c).map(|l| l.iter().filter(|e| saved(e)).map(|e| e.1.clone()));
            let ents: Vec<Nbt> = kept.into_iter().flatten().chain(fresh).collect();
            return crate::nbtio::encode_root(&root(ents));
        };
        let lent: Vec<bool> = list.iter().map(saved).collect();
        let taken = list.iter_mut().zip(&lent).filter(|(_, lent)| **lent);
        let ents: Vec<Nbt> = taken.map(|(e, _)| std::mem::replace(&mut e.1, Nbt::Byte(0))).chain(fresh).collect();
        let root = root(ents);
        let bytes = crate::nbtio::encode_root(&root);
        // The lent compounds back in their slots, in the same order.
        if let Nbt::Compound(mut fields) = root
            && let Some((_, Nbt::List(ents))) = fields.pop()
        {
            let slots = list.iter_mut().zip(&lent).filter(|(_, lent)| **lent);
            for ((e, _), back) in slots.zip(ents) {
                e.1 = back;
            }
        }
        bytes
    }

    /// Metadata and equipment of an entity read from the world (`entities.md` § 3): common
    /// fields, mob (no AI, left-handed), age, ground item, worn equipment.
    pub(crate) fn send_entity_meta(&self, id: i32, e: &rc_proto::nbt::Nbt) {
        self.send_entity_meta_after(id, e, None);
    }

    /// [`Server::send_entity_meta`] after a modification of the compound `old` (`/data`): the own
    /// data of displays and interactions are sent when they changed.
    pub(crate) fn send_entity_meta_after(&self, id: i32, e: &rc_proto::nbt::Nbt, old: Option<&rc_proto::nbt::Nbt>) {
        use crate::nbtio::int;
        use rc_proto::nbt::Nbt;
        // Never announced (marker): no data either.
        if !crate::decoration::client_visible(e) {
            return;
        }
        let flag = |k: &str| int(e, k).is_some_and(|v| v != 0);
        let mut values: Vec<(u8, Meta)> = Vec::new();
        let shared = shared_flags(e, int(e, "Fire").is_some_and(|f| f > 0));
        if shared != 0 {
            values.push((0, Meta::Byte(shared)));
        }
        if let Some(a) = int(e, "Air").filter(|&a| a != 300) {
            values.push((1, Meta::Int(a as i32)));
        }
        if let Some(n) = e.get("CustomName") {
            values.push((2, Meta::Text(Some(n.clone()))));
        }
        if flag("CustomNameVisible") {
            values.push((3, Meta::Bool(true)));
        }
        if flag("Silent") {
            values.push((4, Meta::Bool(true)));
        }
        if flag("NoGravity") {
            values.push((5, Meta::Bool(true)));
        }
        let kind = e.get("id").and_then(Nbt::as_str).unwrap_or("");
        if kind == "minecraft:item" {
            if let Some(s) = e.get("Item").and_then(Stack::from_nbt) {
                values.push((8, Meta::Item(self.net(Some(&s)))));
            }
        } else if e.get("Health").is_some() {
            // Living mob: health, then `Mob` flags and age of the "growing mob" layer.
            if let Some(h) = crate::nbtio::float(e, "Health") {
                values.push((9, Meta::Float(h)));
            }
            if e.get("LeftHanded").is_some() || e.get("PersistenceRequired").is_some() {
                let mut m = 0i8;
                if flag("NoAI") {
                    m |= 1;
                }
                if flag("LeftHanded") {
                    m |= 2;
                }
                if m != 0 {
                    values.push((15, Meta::Byte(m)));
                }
                if let Some(age) = int(e, "Age")
                    && age < 0
                {
                    values.push((16, Meta::Bool(true)));
                }
                if flag("AgeLocked") {
                    values.push((17, Meta::Bool(true)));
                }
            }
        }
        self.deco_meta(e, &mut values); // paintings, frames, stands, cushions, mannequins
        self.display_meta(e, old, &mut values); // G5-F05: displays, interaction
        if !values.is_empty() {
            let _ = self.send(Event::Entity(EntityEvent::Data { id, values }));
        }
        // Equipment (`equipment`: hand, armor).
        if let Some(Nbt::Compound(eq)) = e.get("equipment") {
            let slot_of = |k: &str| match k {
                "mainhand" => Some(0u8),
                "offhand" => Some(1),
                "feet" => Some(2),
                "legs" => Some(3),
                "chest" => Some(4),
                "head" => Some(5),
                "body" => Some(6),
                "saddle" => Some(7),
                _ => None,
            };
            let slots: rc_proto_v777::entity::Equipment =
                eq.iter().filter_map(|(k, v)| Some((slot_of(k)?, self.net(Some(&Stack::from_nbt(v)?))))).collect();
            if !slots.is_empty() {
                let _ = self.send(Event::Entity(EntityEvent::Equipment { id, slots }));
            }
        }
    }

    /// Block broken by the player, just before its removal: contents thrown,
    /// tool wear, exhaustion, loot and experience (survival only).
    pub(crate) fn on_block_broken(&mut self, p: [i32; 3], state: u32) {
        self.piglin_block_destroyed(p); // F13: block guarded by the piglins
        // Contents before the loot of the block, like the game (which throws them at the removal,
        // before that loot); consumed here, the removal then finds nothing.
        self.container_broken(p);
        if self.game_mode == 1 {
            return;
        }
        // The version data held by its own handle: the block's name is lent, not copied.
        let data = self.data.clone();
        let Some(block) = data.block_of(state).map(|b| b.name.as_str()) else { return };
        let held = 36 + self.held as usize;
        let tool = self.slots[held].clone();
        // Wear: non-zero hardness; shears: any block outside `#fire` (G5-F05).
        let hardness = self.data.hardness.get(state as usize).copied().unwrap_or(0.0);
        let shears = tool.as_ref().is_some_and(|t| t.id == "minecraft:shears");
        let wears = if shears {
            let block = self.data.state(state).map_or(u32::MAX, |s| u32::from(s.block));
            !self.block_tags.get("minecraft:fire").is_some_and(|v| v.binary_search(&block).is_ok())
        } else {
            hardness != 0.0
        };
        if wears
            && let Some(t) = &tool
            && let Some(info) = self.life.items.id(&t.id).and_then(|i| self.data.items.get(i as usize))
            && let Some(tc) = &info.tool
        {
            let n = tc.damage_per_block as i32;
            if n > 0 {
                self.hurt_item(held, n);
            }
        }
        self.s9_tool_used(tool.as_ref()); // S9: tool wear after mining
        // Right tool (or block with no requirement): loot, exhaustion, experience.
        let needs = self.data.needs_tool.get(state as usize).copied().unwrap_or(false);
        let correct = !needs || self.tool_correct(tool.as_ref(), state);
        if !correct {
            return;
        }
        self.s9_block_mined(block); // S9: destruction by a player
        self.life.vit.exhaust(0.005, self.abilities().invulnerable);
        let hive = block == "minecraft:beehive" || block == "minecraft:bee_nest";
        // Bees inside when it breaks, read before the loot like the block entity it lends.
        let bees_inside = if hive {
            crate::nbtio::list(self.block_entity(p).and_then(|b| b.get("bees"))).len() as i32
        } else {
            0
        };
        // The loot's draw lends the world to the conditions (location offset of tall grass,
        // breaking player).
        let drops = if crate::loot::loot_context_borrowed() {
            let mined = crate::block_loot::BlockRemoval::Mined { tool: tool.as_ref(), by_player: true };
            self.draw_block_loot(p, state, mined)
        } else {
            self.mined_block_loot_with_owned_context(p, state, block, tool.as_ref(), true)
        };
        for d in drops {
            self.pop_resource(p, d);
        }
        self.drop_block_xp(p, block, tool.as_ref());
        let silk = tool.as_ref().is_some_and(|t| self.life.items.enchantment(t, "minecraft:silk_touch") > 0);
        self.infested_broken(p, block, tool.as_ref()); // F01: silverfish of infested blocks
        self.hive_broken(p, block, tool.as_ref()); // F11: bees released, anger
        // W8c: `bee_nest_destroyed`: bees left inside (Silk Touch: all).
        if hive {
            let bees = if silk { bees_inside } else { 0 };
            self.trig_bee_nest(state, tool.as_ref(), bees);
        }
    }

    /// Experience of a broken block: draw, Silk Touch that cancels it
    /// (except for the spawner), then the loot rule.
    pub(crate) fn drop_block_xp(&mut self, p: [i32; 3], block: &str, tool: Option<&Stack>) {
        let silk = tool.is_some_and(|t| self.life.items.enchantment(t, "minecraft:silk_touch") > 0);
        let xp = match block_xp(block) {
            Some(BlockXp::Range(a, b)) => {
                let n = self.life.rng.int_between(a, b);
                if silk { 0 } else { n }
            }
            Some(BlockXp::Const(n)) => {
                if silk {
                    0
                } else {
                    n
                }
            }
            Some(BlockXp::Spawner) => {
                let r = &mut self.life.rng;
                15 + r.next_int_bound(15) + r.next_int_bound(15)
            }
            None => 0,
        };
        if xp > 0 && self.rule("minecraft:block_drops", true) {
            self.spawn_orbs([f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5], xp);
        }
    }

    pub(crate) fn tool_correct(&self, tool: Option<&Stack>, state: u32) -> bool {
        let block = self.data.state(state).map_or(u32::MAX, |s| u32::from(s.block));
        let Some(t) = tool
            .and_then(|s| self.life.items.id(&s.id))
            .and_then(|i| self.data.items.get(i as usize))
            .and_then(|i| i.tool.as_ref())
        else {
            return false;
        };
        t.rules
            .iter()
            .find(|r| r.correct.is_some() && self.tag_contains(&r.blocks, block))
            .and_then(|r| r.correct)
            .unwrap_or(false)
    }

    fn tag_contains(&self, set: &rc_data::BlockSet, block: u32) -> bool {
        match set {
            rc_data::BlockSet::Tag(t) => self.block_tags.get(t).is_some_and(|v| v.binary_search(&block).is_ok()),
            rc_data::BlockSet::Blocks(b) => b.contains(&block),
        }
    }

    /// Wear of an item of menu 0: Unbreaking, breaking.
    /// Remaining wear after Unbreaking: each point is avoided with the chance of the enchantment
    /// (armor: 2n / (5n + 5), other: n / (n + 1)), world's random draw.
    pub(crate) fn unbreaking_kept(&mut self, s: &Stack, n: i32, armor: bool) -> i32 {
        let unbreaking = self.life.items.enchantment(s, "minecraft:unbreaking");
        if unbreaking <= 0 {
            return n;
        }
        let u = unbreaking as f32;
        let p = if armor { 2.0 * u / (5.0 * u + 5.0) } else { u / (u + 1.0) };
        let mut removed = 0;
        for _ in 0..n {
            if self.life.rng.next_float() < p {
                removed += 1;
            }
        }
        n - removed
    }

    pub(crate) fn hurt_item(&mut self, slot: usize, n: i32) {
        if self.game_mode == 1 {
            return;
        }
        let Some(s) = self.slots[slot].clone() else { return };
        let items = &self.life.items;
        if !items.damageable(&s) {
            return;
        }
        // Formula chosen by the item (enchantable armor tag), not by the slot: an armor piece held
        // in hand wears like armor, elytra worn on the chest like an ordinary item.
        let armor = items.has_tag("minecraft:enchantable/armor", &s.id);
        let n2 = self.unbreaking_kept(&s, n, armor);
        if n2 <= 0 {
            return;
        }
        let items = &self.life.items;
        let max = items.max_damage(&s);
        let dmg = items.damage(&s) + n2;
        self.s9_item_damaged(&s, dmg.min(max), dmg >= max); // S9
        let mut s = s;
        if dmg >= max {
            // Broken: stack −1, break sound; the rest of the stack keeps the wear, capped at the
            // durability (so it is broken too).
            s.count -= 1;
            crate::items::set_component(&mut s, "minecraft:damage", rc_proto::nbt::Nbt::Int(max));
            self.slots[slot] = (s.count > 0).then_some(s);
            self.use_stack_emptied(slot); // bow, brush, shield broken mid-use
            // W8c: break event specific to the slot (47 main hand, 48 off hand, 49-52
            // head, chest, legs, feet), like the equipment slot.
            let event = match slot {
                45 => 48,
                5 => 49,
                6 => 50,
                7 => 51,
                8 => 52,
                _ => 47,
            };
            let _ = self.send(Event::Entity(EntityEvent::Event { id: self.player_id, event }));
        } else {
            crate::items::set_component(&mut s, "minecraft:damage", rc_proto::nbt::Nbt::Int(dmg));
            self.slots[slot] = Some(s);
        }
    }
}

// ---------- F03: ground items taken by a mob (sulfur cube) ----------

impl ItemEnt {
    /// Removed from the world (taken or destroyed) but not yet forgotten.
    pub(crate) fn is_removed(&self) -> bool {
        self.removed
    }

    /// Removed from the world (poured into a hopper): disappears at the end of the item tick.
    pub(crate) fn mark_removed(&mut self) {
        self.removed = true;
    }
}

impl Orb {
    /// S8: removed (merge of thrown orbs).
    pub(crate) fn is_removed(&self) -> bool {
        self.removed
    }

    /// Removed from the world (`kill` during the entity walk): leaves with the walk's removals.
    pub(crate) fn mark_removed(&mut self) {
        self.removed = true;
    }

    /// Bench of the merging: the orb is back for the next round.
    #[cfg(test)]
    pub(crate) fn unremove_for_bench(&mut self) {
        self.removed = false;
    }

    /// Push (explosion blast): velocity sent back at the next tracking, at once if
    /// the orb has just been hurt.
    pub(crate) fn push_sync(&mut self, kb: [f64; 3], hurt: bool) {
        self.vel = [self.vel[0] + kb[0], self.vel[1] + kb[1], self.vel[2] + kb[2]];
        self.track.needs_sync = true;
        self.track.sync_velocity |= hurt;
    }
}

impl ItemEnt {
    /// Push (explosion blast): as for the orb.
    pub(crate) fn push_sync(&mut self, kb: [f64; 3], hurt: bool) {
        self.vel = [self.vel[0] + kb[0], self.vel[1] + kb[1], self.vel[2] + kb[2]];
        self.track.needs_sync = true;
        self.track.sync_velocity |= hurt;
    }
}

impl Server {
    /// A mob takes one copy of ground item `item` (`split(1)` then `take`); the emptied stack
    /// disappears at the next item tick, as in the game.
    /// F05: a mob takes `n` copies (inventory pickup): a single "taken" packet, the emptied
    /// stack disappears.
    pub(crate) fn mob_take(&mut self, item: i32, collector: i32, n: i32) {
        let Some(d) = self.life.drops.iter_mut().find(|d| d.id == item && !d.removed) else { return };
        let n = n.min(d.stack.count);
        d.stack.count -= n;
        let left = d.stack.count;
        if left <= 0 {
            d.removed = true;
        }
        let _ = self.send(Event::Misc(rc_client::misc::Misc::Take { item, collector, amount: n }));
        if left > 0
            && let Some(d) = self.life.drops.iter().find(|d| d.id == item)
        {
            let net = self.net(Some(&d.stack));
            let _ = self.send(Event::Entity(EntityEvent::Data { id: item, values: vec![(8, Meta::Item(net))] }));
        }
    }

    /// A mob empties ground item `item` at once: the pickup packet carries `amount` (what the
    /// game counts at that moment, which may be nothing), then the item is gone whatever is left.
    pub(crate) fn mob_take_whole(&mut self, item: i32, collector: i32, amount: i32) {
        let Some(d) = self.life.drops.iter_mut().find(|d| d.id == item && !d.removed) else { return };
        d.removed = true;
        let _ = self.send(Event::Misc(rc_client::misc::Misc::Take { item, collector, amount }));
    }

    /// The stack of ground item `item` cut down to `count` in place, as a split of the game does
    /// (no packet: the item is about to be taken).
    pub(crate) fn set_ground_item_count(&mut self, item: i32, count: i32) {
        if let Some(d) = self.life.drops.iter_mut().find(|d| d.id == item && !d.removed) {
            d.stack.count = count;
        }
    }

    pub(crate) fn mob_take_one(&mut self, item: i32, collector: i32) -> Option<Stack> {
        let d = self.life.drops.iter_mut().find(|d| d.id == item && !d.removed)?;
        if d.stack.count <= 0 {
            return None;
        }
        let one = Stack { count: 1, ..d.stack.clone() };
        d.stack.count -= 1;
        let left = d.stack.count;
        let _ = self.send(Event::Misc(rc_client::misc::Misc::Take { item, collector, amount: 1 }));
        if left <= 0 {
            if let Some(d) = self.life.drops.iter_mut().find(|d| d.id == item) {
                d.removed = true;
            }
        } else if let Some(d) = self.life.drops.iter().find(|d| d.id == item) {
            let net = self.net(Some(&d.stack));
            let _ = self.send(Event::Entity(EntityEvent::Data { id: item, values: vec![(8, Meta::Item(net))] }));
        }
        Some(one)
    }
}

// ---------- Damage taken by ground items and orbs (`/damage`) ----------

impl Server {
    /// True if the stack resists damage type `ty`: the `types` of its `damage_resistant`
    /// component (patch, otherwise prototype) hold it. Like the game, `types` is a tag
    /// (`#is_fire`), one damage type or a list of damage types.
    pub(crate) fn item_resists(&self, s: &Stack, ty: &str) -> bool {
        if let Some(c) = crate::items::component(s, "minecraft:damage_resistant") {
            return c.get("types").is_some_and(|t| self.nbt_damage_set_has(t, ty));
        }
        self.life
            .items
            .default_component(s, "minecraft:damage_resistant")
            .is_some_and(|v| crate::item_use::damage_set_has(self, &v["types"], ty))
    }

    /// [`crate::item_use::damage_set_has`] for a set written in NBT (component patch).
    fn nbt_damage_set_has(&self, set: &Nbt, ty: &str) -> bool {
        match set {
            Nbt::String(entry) => self.damage_set_entry_has(entry, ty),
            // A list holds damage types only, never tags.
            Nbt::List(entries) => entries.iter().any(|e| e.as_str().is_some_and(|id| same_identifier(ty, id))),
            _ => false,
        }
    }

    /// Damage to a ground item: refused if a mob causes it without `mob_griefing` or if the
    /// stack resists it; otherwise health points removed (the game's integer truncation), item destroyed at 0
    /// with its contents (box, bag) spilled in its place. True if they landed.
    pub(crate) fn item_hurt(&mut self, id: i32, ty: &str, amount: f32, by_mob: bool) -> bool {
        let Some(k) = self.life.drops.iter().position(|d| d.id == id && !d.removed) else { return false };
        self.item_hurt_at(k, ty, amount, by_mob)
    }

    /// [`Server::item_hurt`] on the item of slot `k` of the list (nothing if it is removed).
    pub(crate) fn item_hurt_at(&mut self, k: usize, ty: &str, amount: f32, by_mob: bool) -> bool {
        if self.life.drops.get(k).is_none_or(|d| d.removed) {
            return false;
        }
        if self.invulnerable_to(self.life.drops[k].common, ty) || !self.item_may_hurt(&self.life.drops[k].stack, ty, by_mob) {
            return false;
        }
        let spill = item_take_damage(&mut self.life.drops[k], amount);
        let pos = self.life.drops[k].pos;
        for s in spill {
            self.spawn_item_random(pos, s, 0);
        }
        true
    }

    /// [`Server::item_hurt`] on an item taken out of the list (its own tick).
    pub(crate) fn item_hurt_ent(&mut self, d: &mut ItemEnt, ty: &str, amount: f32, by_mob: bool) -> bool {
        if d.removed || self.invulnerable_to(d.common, ty) || !self.item_may_hurt(&d.stack, ty, by_mob) {
            return false;
        }
        for s in item_take_damage(d, amount) {
            self.spawn_item_random(d.pos, s, 0);
        }
        true
    }

    /// Damage refused: caused by a mob without `mob_griefing`, or resisted by the stack.
    fn item_may_hurt(&self, s: &Stack, ty: &str, by_mob: bool) -> bool {
        !(by_mob && !self.rule("minecraft:mob_griefing", true)) && !self.item_resists(s, ty)
    }

    /// Damage to an experience orb: health points removed, orb removed at 0.
    pub(crate) fn orb_hurt(&mut self, id: i32, ty: &str, amount: f32) -> bool {
        let Some(k) = self.life.orbs.iter().position(|o| o.id == id && !o.removed) else { return false };
        self.orb_hurt_at(k, ty, amount)
    }

    /// [`Server::orb_hurt`] on the orb of slot `k` of the list (nothing if it is removed).
    pub(crate) fn orb_hurt_at(&mut self, k: usize, ty: &str, amount: f32) -> bool {
        if self.life.orbs.get(k).is_none_or(|o| o.removed) {
            return false;
        }
        if self.invulnerable_to(self.life.orbs[k].common, ty) {
            return false;
        }
        orb_take_damage(&mut self.life.orbs[k], amount);
        true
    }

    /// [`Server::orb_hurt`] on an orb taken out of the list (its own tick, the blocks it touches).
    pub(crate) fn orb_hurt_ent(&self, o: &mut Orb, ty: &str, amount: f32) -> bool {
        if self.invulnerable_to(o.common, ty) {
            return false;
        }
        orb_take_damage(o, amount);
        true
    }

    /// `Invulnerable` item or orb: only damage of the `bypasses_invulnerability` tag gets
    /// through. The game also lets a creative player's damage through; the callers here never
    /// carry that source (residue noted in `docs/gaps-protocol.md` § G).
    fn invulnerable_to(&self, c: Common, ty: &str) -> bool {
        (c.invulnerable || c.invulnerable_time > 0) && !self.dmg_tag(ty, "minecraft:bypasses_invulnerability")
    }
}

/// Health points removed (the game's integer truncation); at 0 the item is destroyed and its
/// contents (box, bag) are returned to be spilled in its place.
fn item_take_damage(d: &mut ItemEnt, amount: f32) -> Vec<Stack> {
    use rc_proto::nbt::Nbt;
    d.health = (d.health as f32 - amount) as i32;
    let mut spill = Vec::new();
    if d.health > 0 {
        return spill;
    }
    d.removed = true;
    if let Some(Nbt::List(l)) = crate::items::component(&d.stack, "minecraft:container") {
        spill.extend(l.iter().filter_map(|e| e.get("item").and_then(Stack::from_nbt)));
    }
    if let Some(Nbt::List(l)) = crate::items::component(&d.stack, "minecraft:bundle_contents") {
        spill.extend(l.iter().filter_map(Stack::from_nbt));
    }
    spill
}

/// Health points removed, orb removed at 0.
pub(crate) fn orb_take_damage(o: &mut Orb, amount: f32) {
    o.health = (o.health as f32 - amount) as i32;
    if o.health <= 0 {
        o.removed = true;
    }
}

/// Shared flags (entity data 0) of saved entity `e`: shown burning when `on_fire` or given the
/// visual fire, invisible, glowing.
pub(crate) fn shared_flags(e: &rc_proto::nbt::Nbt, on_fire: bool) -> i8 {
    let flag = |k: &str| crate::nbtio::int(e, k).is_some_and(|v| v != 0);
    let mut shared = 0i8;
    if on_fire || flag("HasVisualFire") {
        shared |= 1;
    }
    if flag("Invisible") {
        shared |= 0x20;
    }
    if flag("Glowing") {
        shared |= 0x40;
    }
    shared
}
