//! Points of interest (F05, `docs/specs/server-mobs.md` § "Generic POI mechanism"):
//! registry per chunk section (position, type, free tickets), updated on each
//! block state change (applied on the next tick, like the game's deferred task), read and
//! written in `poi/` in the game's format, searches within a radius and distance to the village.
//!
//! Search paths (`PoiPaths`), each switch read once, on by default, `=0` for the old path:
//! `RUSTCRAFT_OPT_VILLAGESHELLS` (village distance by growing shells, bounded predicates read
//! only the cube they need), `RUSTCRAFT_OPT_VILLAGEBATCH` (callers that test a whole cube of
//! sections collect the village sections once), `RUSTCRAFT_OPT_POISORTED` (records kept in
//! search order, non-empty sections listed per column), `RUSTCRAFT_OPT_POICOLUMNS` (occupied
//! village records counted per column: empty columns skipped), `RUSTCRAFT_OPT_POIINDEXEDAREAS`
//! (a square of chunks found fully indexed is not checked chunk by chunk again).

use hashbrown::{HashMap, HashSet};

use rc_proto::nbt::Nbt;

use crate::server::Server;

/// Chunks indexed around a village distance test (±7, the game's radius): never reduced, the
/// indexing order and the chunks marked for saving depend on it.
const VILLAGE_INDEX_RADIUS: i32 = 7;
/// Farthest village distance looked at; beyond, the distance reads `VILLAGE_FAR`.
const VILLAGE_SEARCH_RADIUS: i32 = 6;
const VILLAGE_FAR: i32 = 7;

/// Which search paths the registry takes (see the module comment for each switch).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PoiPaths {
    pub village_shells: bool,
    pub village_batch: bool,
    pub sorted_records: bool,
    pub village_columns: bool,
    pub indexed_areas: bool,
}

#[cfg(test)]
thread_local! {
    /// Tests: every new path of the village, golem and raid searches turned off on this thread
    /// (registries created afterwards, and the switches of the golem and illager modules).
    pub(crate) static FORCE_OLD_SEARCHES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A new path's switch, unless a test forced the old searches on this thread.
pub(crate) fn new_search_path(switch: bool) -> bool {
    #[cfg(test)]
    if FORCE_OLD_SEARCHES.get() {
        return false;
    }
    switch
}

#[cfg(test)]
thread_local! {
    /// `None` until `start_noting_search_paths` (the benches never count).
    static SEARCH_PATH_USES: std::cell::RefCell<Option<std::collections::BTreeMap<&'static str, u64>>> =
        const { std::cell::RefCell::new(None) };
}

/// Tests: notes that a new search path ran on this thread, so that an equivalence proof can show
/// its scenario went through it. Nothing outside the tests.
#[inline]
pub(crate) fn note_search_path(_name: &'static str) {
    #[cfg(test)]
    SEARCH_PATH_USES.with(|u| {
        if let Some(uses) = u.borrow_mut().as_mut() {
            *uses.entry(_name).or_default() += 1;
        }
    });
}

/// Tests: counts the new search paths run on this thread from now on.
#[cfg(test)]
pub(crate) fn start_noting_search_paths() {
    SEARCH_PATH_USES.set(Some(std::collections::BTreeMap::new()));
}

/// Tests: the new search paths run on this thread since `start_noting_search_paths`, with their
/// counts.
#[cfg(test)]
pub(crate) fn search_path_uses() -> std::collections::BTreeMap<&'static str, u64> {
    SEARCH_PATH_USES.with(|u| u.borrow().clone().unwrap_or_default())
}

impl PoiPaths {
    #[cfg(test)]
    pub(crate) const OLD: PoiPaths = PoiPaths {
        village_shells: false,
        village_batch: false,
        sorted_records: false,
        village_columns: false,
        indexed_areas: false,
    };

    pub(crate) fn from_switches() -> PoiPaths {
        PoiPaths {
            village_shells: new_search_path(rc_data::opt_switch!("RUSTCRAFT_OPT_VILLAGESHELLS")),
            village_batch: new_search_path(rc_data::opt_switch!("RUSTCRAFT_OPT_VILLAGEBATCH")),
            sorted_records: new_search_path(rc_data::opt_switch!("RUSTCRAFT_OPT_POISORTED")),
            village_columns: new_search_path(rc_data::opt_switch!("RUSTCRAFT_OPT_POICOLUMNS")),
            indexed_areas: new_search_path(rc_data::opt_switch!("RUSTCRAFT_OPT_POIINDEXEDAREAS")),
        }
    }
}

impl Default for PoiPaths {
    fn default() -> PoiPaths {
        PoiPaths::from_switches()
    }
}

/// POI types (registry `point_of_interest_type`, the game's registration order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PoiType {
    Armorer,
    Butcher,
    Cartographer,
    Cleric,
    Farmer,
    Fisherman,
    Fletcher,
    Leatherworker,
    Librarian,
    Mason,
    Shepherd,
    Toolsmith,
    Weaponsmith,
    Home,
    Meeting,
    Beehive,
    BeeNest,
    NetherPortal,
    Lodestone,
    TestInstance,
    LightningRod,
}

pub const ALL: [PoiType; 21] = [
    PoiType::Armorer,
    PoiType::Butcher,
    PoiType::Cartographer,
    PoiType::Cleric,
    PoiType::Farmer,
    PoiType::Fisherman,
    PoiType::Fletcher,
    PoiType::Leatherworker,
    PoiType::Librarian,
    PoiType::Mason,
    PoiType::Shepherd,
    PoiType::Toolsmith,
    PoiType::Weaponsmith,
    PoiType::Home,
    PoiType::Meeting,
    PoiType::Beehive,
    PoiType::BeeNest,
    PoiType::NetherPortal,
    PoiType::Lodestone,
    PoiType::TestInstance,
    PoiType::LightningRod,
];

impl PoiType {
    pub fn name(self) -> &'static str {
        match self {
            PoiType::Armorer => "minecraft:armorer",
            PoiType::Butcher => "minecraft:butcher",
            PoiType::Cartographer => "minecraft:cartographer",
            PoiType::Cleric => "minecraft:cleric",
            PoiType::Farmer => "minecraft:farmer",
            PoiType::Fisherman => "minecraft:fisherman",
            PoiType::Fletcher => "minecraft:fletcher",
            PoiType::Leatherworker => "minecraft:leatherworker",
            PoiType::Librarian => "minecraft:librarian",
            PoiType::Mason => "minecraft:mason",
            PoiType::Shepherd => "minecraft:shepherd",
            PoiType::Toolsmith => "minecraft:toolsmith",
            PoiType::Weaponsmith => "minecraft:weaponsmith",
            PoiType::Home => "minecraft:home",
            PoiType::Meeting => "minecraft:meeting",
            PoiType::Beehive => "minecraft:beehive",
            PoiType::BeeNest => "minecraft:bee_nest",
            PoiType::NetherPortal => "minecraft:nether_portal",
            PoiType::Lodestone => "minecraft:lodestone",
            PoiType::TestInstance => "minecraft:test_instance",
            PoiType::LightningRod => "minecraft:lightning_rod",
        }
    }

    pub fn from_name(n: &str) -> Option<PoiType> {
        let n = if n.contains(':') { n.to_owned() } else { format!("minecraft:{n}") };
        ALL.iter().copied().find(|t| t.name() == n)
    }

    pub fn max_tickets(self) -> i32 {
        match self {
            PoiType::Meeting => 32,
            PoiType::Beehive
            | PoiType::BeeNest
            | PoiType::NetherPortal
            | PoiType::Lodestone
            | PoiType::TestInstance
            | PoiType::LightningRod => 0,
            _ => 1,
        }
    }

    /// Validity radius (tolerance of the acquisition path).
    pub fn valid_range(self) -> i32 {
        if self == PoiType::Meeting { 6 } else { 1 }
    }

    /// Tag `acquirable_job_site`: the thirteen workstations.
    pub fn is_job(self) -> bool {
        (self as u8) <= PoiType::Weaponsmith as u8
    }

    /// Tag `village`: workstations, beds, bells.
    pub fn is_village(self) -> bool {
        (self as u8) <= PoiType::Meeting as u8
    }
}

/// POI type of a block (name, properties): bed = head only, cauldrons and lightning rods
/// in all their forms.
fn type_of_block(name: &str, props: &[(&str, &str)]) -> Option<PoiType> {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    Some(match n {
        "blast_furnace" => PoiType::Armorer,
        "smoker" => PoiType::Butcher,
        "cartography_table" => PoiType::Cartographer,
        "brewing_stand" => PoiType::Cleric,
        "composter" => PoiType::Farmer,
        "barrel" => PoiType::Fisherman,
        "fletching_table" => PoiType::Fletcher,
        "cauldron" | "water_cauldron" | "lava_cauldron" | "powder_snow_cauldron" => PoiType::Leatherworker,
        "lectern" => PoiType::Librarian,
        "stonecutter" => PoiType::Mason,
        "loom" => PoiType::Shepherd,
        "smithing_table" => PoiType::Toolsmith,
        "grindstone" => PoiType::Weaponsmith,
        "bell" => PoiType::Meeting,
        "beehive" => PoiType::Beehive,
        "bee_nest" => PoiType::BeeNest,
        "nether_portal" => PoiType::NetherPortal,
        "lodestone" => PoiType::Lodestone,
        "test_instance_block" => PoiType::TestInstance,
        // Only the 16 coloured beds are homes: the straw bed is not one.
        _ if n.ends_with("_bed") && n != "straw_bed" => {
            if props.iter().any(|(k, v)| *k == "part" && *v == "head") {
                PoiType::Home
            } else {
                return None;
            }
        }
        _ if n.ends_with("lightning_rod") => PoiType::LightningRod,
        _ => return None,
    })
}

/// Record: position, type, free tickets, insertion rank (game order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    pub pos: [i32; 3],
    pub ty: PoiType,
    pub free: i32,
    seq: u64,
}

impl Record {
    pub fn has_space(&self) -> bool {
        self.free > 0
    }

    pub fn occupied(&self) -> bool {
        self.free != self.ty.max_tickets()
    }
}

/// Occupancy filter of the searches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Occupancy {
    HasSpace,
    IsOccupied,
    Any,
}

impl Occupancy {
    fn test(self, r: &Record) -> bool {
        match self {
            Occupancy::HasSpace => r.has_space(),
            Occupancy::IsOccupied => r.occupied(),
            Occupancy::Any => true,
        }
    }
}

#[derive(Debug, Default, Clone)]
struct Section {
    /// Insertion order (the order of the saved file).
    records: Vec<Record>,
    /// Indices of `records` in the order of the radius searches: type, then hash bucket among the
    /// records of that type, then insertion rank. Rebuilt when a record comes or goes, so that a
    /// search no longer sorts at every call.
    search_order: Vec<u32>,
    valid: bool,
}

impl Section {
    fn new(records: Vec<Record>, valid: bool) -> Section {
        let mut s = Section { records, search_order: Vec::new(), valid };
        s.rebuild_search_order();
        s
    }

    fn rebuild_search_order(&mut self) {
        let mut per_type = [0usize; ALL.len()];
        for r in &self.records {
            per_type[r.ty as usize] += 1;
        }
        let records = &self.records;
        let mut order: Vec<u32> = (0..records.len() as u32).collect();
        order.sort_by_key(|&i| {
            let r = &records[i as usize];
            (r.ty, java_bucket(r.pos, per_type[r.ty as usize]), r.seq)
        });
        self.search_order = order;
    }

    fn has_occupied_village(&self) -> bool {
        self.records.iter().any(|r| r.ty.is_village() && r.occupied())
    }

    fn summary(&self) -> SectionSummary {
        let occupied_village = self.records.iter().filter(|r| r.ty.is_village() && r.occupied()).count() as u32;
        SectionSummary { filled: !self.records.is_empty(), occupied_village }
    }
}

/// What the column summary keeps of one section.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct SectionSummary {
    filled: bool,
    occupied_village: u32,
}

/// Summary of the sections of one chunk column.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ColumnSummary {
    /// Height (section y) of the sections holding at least one record, ascending.
    filled_sections: Vec<i32>,
    /// Occupied village records of the column.
    occupied_village: u32,
}

/// State change pending: position, old type, new type.
type Change = ([i32; 3], Option<PoiType>, Option<PoiType>);

/// Section read from `poi/`: section (x, y, z) key.
type DiskSection = ((i32, i32, i32), Section);

/// POI registry of a dimension.
#[derive(Debug, Default)]
pub struct Pois {
    /// Search paths, read from the switches when the registry is made.
    pub(crate) paths: PoiPaths,
    /// Type per state identifier (0 = none, otherwise type + 1), built on first use.
    table: Vec<u8>,
    sections: HashMap<(i32, i32, i32), Section>,
    /// Per chunk column (x, z): its filled sections and occupied village records, kept in
    /// step with `sections` by every change.
    columns: HashMap<(i32, i32), ColumnSummary>,
    /// Indexed chunks (read from disk or scanned). Never forgotten.
    indexed: HashSet<(i32, i32)>,
    /// Squares (centre chunk x, z, radius) whose chunks were all found indexed: since `indexed`
    /// only grows, they stay so and are not checked again (`RUSTCRAFT_OPT_POIINDEXEDAREAS`).
    indexed_squares: HashSet<(i32, i32, i32)>,
    /// Sections read from `poi/`, waiting for the block chunk.
    from_disk: HashMap<(i32, i32), Vec<DiskSection>>,
    /// State changes pending (position, old type, new type).
    pending: Vec<Change>,
    /// Chunks to rewrite into `poi/`.
    dirty: HashSet<(i32, i32)>,
    seq: u64,
}

fn section_key(p: [i32; 3]) -> (i32, i32, i32) {
    (p[0] >> 4, p[1] >> 4, p[2] >> 4)
}

/// Iteration rank of a hashed set of the game's points of interest: hash bucket of the position (capacity
/// depending on the number of elements), then insertion order.
fn java_bucket(p: [i32; 3], n: usize) -> u32 {
    let h = (p[1].wrapping_add(p[2].wrapping_mul(31))).wrapping_mul(31).wrapping_add(p[0]);
    let h = (h ^ ((h as u32) >> 16) as i32) as u32;
    let mut cap = 16usize;
    while n > cap * 3 / 4 {
        cap *= 2;
    }
    h & (cap as u32 - 1)
}

impl Pois {
    fn summary_of(&self, key: (i32, i32, i32)) -> SectionSummary {
        self.sections.get(&key).map(Section::summary).unwrap_or_default()
    }

    /// Brings the summary of the column of section `key` in step after that section went from
    /// `before` to its current state.
    fn note_section_change(&mut self, key: (i32, i32, i32), before: SectionSummary) {
        let after = self.summary_of(key);
        if after == before {
            return;
        }
        let column = self.columns.entry((key.0, key.2)).or_default();
        column.occupied_village = column.occupied_village + after.occupied_village - before.occupied_village;
        let at = column.filled_sections.binary_search(&key.1);
        match (at, after.filled) {
            (Err(i), true) => column.filled_sections.insert(i, key.1),
            (Ok(i), false) => {
                column.filled_sections.remove(i);
            }
            _ => {}
        }
        if column.filled_sections.is_empty() && column.occupied_village == 0 {
            self.columns.remove(&(key.0, key.2));
        }
    }

    /// Puts a whole section in place (chunk indexing).
    fn insert_section(&mut self, key: (i32, i32, i32), section: Section) {
        let before = self.summary_of(key);
        self.sections.insert(key, section);
        self.note_section_change(key, before);
    }

    fn add_record(&mut self, p: [i32; 3], ty: PoiType, free: i32) -> bool {
        self.seq += 1;
        let seq = self.seq;
        let key = section_key(p);
        let before = self.summary_of(key);
        let s = self.sections.entry(key).or_insert_with(|| Section::new(Vec::new(), true));
        if let Some(r) = s.records.iter().find(|r| r.pos == p) {
            if r.ty == ty {
                return false;
            }
            s.records.retain(|r| r.pos != p);
        }
        s.records.push(Record { pos: p, ty, free, seq });
        s.rebuild_search_order();
        self.note_section_change(key, before);
        self.dirty.insert((p[0] >> 4, p[2] >> 4));
        true
    }

    fn remove_record(&mut self, p: [i32; 3]) {
        let key = section_key(p);
        let before = self.summary_of(key);
        if let Some(s) = self.sections.get_mut(&key) {
            let count = s.records.len();
            s.records.retain(|r| r.pos != p);
            if s.records.len() != count {
                s.rebuild_search_order();
                self.note_section_change(key, before);
                self.dirty.insert((p[0] >> 4, p[2] >> 4));
            }
        }
    }

    fn record_mut(&mut self, p: [i32; 3]) -> Option<&mut Record> {
        self.sections.get_mut(&section_key(p))?.records.iter_mut().find(|r| r.pos == p)
    }

    pub fn record(&self, p: [i32; 3]) -> Option<&Record> {
        self.sections.get(&section_key(p))?.records.iter().find(|r| r.pos == p)
    }

    pub fn get_type(&self, p: [i32; 3]) -> Option<PoiType> {
        self.record(p).map(|r| r.ty)
    }

    /// Has chunk `c` already been indexed (its POIs are known, even if unloaded since)?
    pub(crate) fn is_indexed(&self, c: (i32, i32)) -> bool {
        self.indexed.contains(&c)
    }

    /// Releases a ticket (`release`); false if already at the maximum.
    pub fn release(&mut self, p: [i32; 3]) -> bool {
        let before = self.summary_of(section_key(p));
        let ok = match self.record_mut(p) {
            Some(r) if r.free < r.ty.max_tickets() => {
                r.free += 1;
                true
            }
            _ => false,
        };
        // A ticket changes the occupancy, never the search order.
        self.note_section_change(section_key(p), before);
        self.dirty.insert((p[0] >> 4, p[2] >> 4));
        ok
    }

    /// Takes a ticket; false if none remain.
    pub fn acquire(&mut self, p: [i32; 3]) -> bool {
        let before = self.summary_of(section_key(p));
        let ok = match self.record_mut(p) {
            Some(r) if r.free > 0 => {
                r.free -= 1;
                true
            }
            _ => false,
        };
        if ok {
            self.note_section_change(section_key(p), before);
            self.dirty.insert((p[0] >> 4, p[2] >> 4));
        }
        ok
    }

    /// Distance (in sections) to the nearest village centre, 7 beyond 6.
    pub fn sections_to_village(&self, s: (i32, i32, i32)) -> i32 {
        if self.paths.village_columns {
            self.village_distance_by_columns(s)
        } else if self.paths.village_shells {
            self.village_distance_by_shells(s)
        } else {
            self.village_distance_full_cube(s)
        }
    }

    /// Is a village centre within `d` sections of section `s` (`sections_to_village(s) <= d`)?
    pub fn within_sections_of_village(&self, s: (i32, i32, i32), d: i32) -> bool {
        if d < 0 {
            return false;
        }
        if d >= VILLAGE_FAR {
            // The distance reads 7 at most.
            return true;
        }
        if self.paths.village_columns {
            note_search_path("bounded village test by columns");
            self.village_within_by_columns(s, d)
        } else if self.paths.village_shells {
            self.village_within_cube(s, d)
        } else {
            self.village_distance_full_cube(s) <= d
        }
    }

    /// Does section `s` hold an occupied village record (distance 0)?
    pub(crate) fn section_has_occupied_village(&self, s: (i32, i32, i32)) -> bool {
        self.sections.get(&s).is_some_and(Section::has_occupied_village)
    }

    /// Old path: the whole cube of ±6 sections read, nearest kept.
    fn village_distance_full_cube(&self, s: (i32, i32, i32)) -> i32 {
        let mut best = VILLAGE_FAR;
        for dx in -6..=6 {
            for dy in -6..=6 {
                for dz in -6..=6 {
                    let d = i32::max(i32::abs(dx), i32::max(i32::abs(dy), i32::abs(dz)));
                    if d >= best {
                        continue;
                    }
                    if self.section_has_occupied_village((s.0 + dx, s.1 + dy, s.2 + dz)) {
                        best = d;
                    }
                }
            }
        }
        best
    }

    /// Shells of growing Chebyshev distance d = 0..6, stopping at the first one that holds a
    /// village: the same minimum as the whole cube, without reading what lies beyond it.
    fn village_distance_by_shells(&self, s: (i32, i32, i32)) -> i32 {
        (0..=VILLAGE_SEARCH_RADIUS).find(|&d| self.shell_has_village(s, d)).unwrap_or(VILLAGE_FAR)
    }

    /// The cells at exactly Chebyshev distance `d` from `s` (the faces of the cube of side 2d + 1).
    fn shell_has_village(&self, s: (i32, i32, i32), d: i32) -> bool {
        let at = |dx: i32, dy: i32, dz: i32| self.section_has_occupied_village((s.0 + dx, s.1 + dy, s.2 + dz));
        if d == 0 {
            return at(0, 0, 0);
        }
        let side = -d..=d;
        let inner = -d + 1..d;
        // The two x faces whole, then the two y faces without their x edges, then the two z
        // faces without their x and y edges: each cell of the shell once.
        let x_faces = || [-d, d].into_iter().any(|dx| side.clone().any(|dy| side.clone().any(|dz| at(dx, dy, dz))));
        let y_faces = || inner.clone().any(|dx| [-d, d].into_iter().any(|dy| side.clone().any(|dz| at(dx, dy, dz))));
        let z_faces = || inner.clone().any(|dx| inner.clone().any(|dy| at(dx, dy, -d) || at(dx, dy, d)));
        x_faces() || y_faces() || z_faces()
    }

    /// Bounded predicate: the cube of side 2d + 1 only, stopping at the first village.
    fn village_within_cube(&self, s: (i32, i32, i32), d: i32) -> bool {
        (-d..=d).any(|dx| {
            (-d..=d).any(|dy| (-d..=d).any(|dz| self.section_has_occupied_village((s.0 + dx, s.1 + dy, s.2 + dz))))
        })
    }

    /// Village sections of the cube of radius `d` around `s`, reached through the columns that
    /// count occupied village records (most columns of the world have none).
    fn village_sections_near(&self, s: (i32, i32, i32), d: i32) -> impl Iterator<Item = (i32, i32, i32)> + '_ {
        (-d..=d).flat_map(move |dz| (-d..=d).map(move |dx| (s.0 + dx, s.2 + dz))).flat_map(move |(cx, cz)| {
            let column = self.columns.get(&(cx, cz)).filter(|c| c.occupied_village > 0);
            let heights = column.map_or(&[][..], |c| &c.filled_sections[..]);
            heights
                .iter()
                .copied()
                .filter(move |&sy| (sy - s.1).abs() <= d)
                .map(move |sy| (cx, sy, cz))
                .filter(|&key| self.section_has_occupied_village(key))
        })
    }

    /// Columns taken by rings of growing horizontal distance: once the nearest village found is
    /// no farther than the next ring, no column beyond can bring a nearer one.
    fn village_distance_by_columns(&self, s: (i32, i32, i32)) -> i32 {
        let mut best = VILLAGE_FAR;
        for ring in 0..=VILLAGE_SEARCH_RADIUS {
            if best <= ring {
                break;
            }
            for (cx, cz) in ring_columns((s.0, s.2), ring) {
                let Some(column) = self.columns.get(&(cx, cz)).filter(|c| c.occupied_village > 0) else { continue };
                for &sy in &column.filled_sections {
                    let d = ring.max((sy - s.1).abs());
                    if d < best && self.section_has_occupied_village((cx, sy, cz)) {
                        best = d;
                    }
                }
            }
        }
        best
    }

    fn village_within_by_columns(&self, s: (i32, i32, i32), d: i32) -> bool {
        self.village_sections_near(s, d).next().is_some()
    }

    /// Village sections inside the cube of `radius` sections around `center`, in no particular
    /// order: one collection that serves the distance of every section of a smaller cube
    /// (`village_distance_among`).
    pub(crate) fn village_sections_in_cube(&self, center: (i32, i32, i32), radius: i32) -> Vec<(i32, i32, i32)> {
        if self.paths.village_columns {
            return self.village_sections_near(center, radius).collect();
        }
        let mut out = Vec::new();
        for dz in -radius..=radius {
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let key = (center.0 + dx, center.1 + dy, center.2 + dz);
                    if self.section_has_occupied_village(key) {
                        out.push(key);
                    }
                }
            }
        }
        out
    }
}

/// A radius search: centre block, radius in blocks, occupancy filter, section heights of the
/// dimension.
pub(crate) struct RangeQuery {
    pub center: [i32; 3],
    pub radius: i32,
    pub occ: Occupancy,
    pub heights: std::ops::RangeInclusive<i32>,
}

impl RangeQuery {
    /// Square then disc (distance² to the centre of the origin block ≤ r²), occupancy filter.
    fn keeps(&self, r: &Record) -> bool {
        let (center, radius) = (self.center, self.radius);
        let (dx, dz) = (r.pos[0] - center[0], r.pos[2] - center[2]);
        self.occ.test(r)
            && dx.abs() <= radius
            && dz.abs() <= radius
            && dist2i(r.pos, center) <= i64::from(radius) * i64::from(radius)
    }

    /// Chunk columns of the search, rows of z, x fastest.
    fn columns(&self) -> impl Iterator<Item = (i32, i32)> {
        let cr = self.radius.div_euclid(16) + 1;
        let (ccx, ccz) = (self.center[0] >> 4, self.center[2] >> 4);
        (ccz - cr..=ccz + cr).flat_map(move |cz| (ccx - cr..=ccx + cr).map(move |cx| (cx, cz)))
    }
}

impl Pois {
    /// Records of the indexed sections matching `pred` and the query, in the game's order:
    /// chunks (x then z), sections from bottom to top, then the order of the game's sets.
    pub(crate) fn records_in_range(&self, pred: impl Fn(PoiType) -> bool, q: &RangeQuery) -> Vec<Record> {
        if self.paths.sorted_records {
            self.records_in_range_presorted(pred, q)
        } else {
            self.records_in_range_sorting_each_call(pred, q)
        }
    }

    /// Only the filled sections of each column, each read in its kept search order.
    fn records_in_range_presorted(&self, pred: impl Fn(PoiType) -> bool, q: &RangeQuery) -> Vec<Record> {
        note_search_path("range search in kept order");
        let mut out = Vec::new();
        for (cx, cz) in q.columns() {
            let Some(column) = self.columns.get(&(cx, cz)) else { continue };
            for &sy in column.filled_sections.iter().filter(|sy| q.heights.contains(sy)) {
                let Some(sec) = self.sections.get(&(cx, sy, cz)) else { continue };
                let in_order = sec.search_order.iter().map(|&i| &sec.records[i as usize]);
                out.extend(in_order.filter(|r| pred(r.ty) && q.keeps(r)).copied());
            }
        }
        out
    }

    /// Old path: every section of every column looked up, its types and records sorted again.
    fn records_in_range_sorting_each_call(&self, pred: impl Fn(PoiType) -> bool, q: &RangeQuery) -> Vec<Record> {
        let mut out = Vec::new();
        for (cx, cz) in q.columns() {
            for sy in q.heights.clone() {
                let Some(sec) = self.sections.get(&(cx, sy, cz)) else { continue };
                let mut by_type: Vec<PoiType> = sec.records.iter().map(|r| r.ty).collect();
                by_type.sort();
                by_type.dedup();
                for ty in by_type {
                    if !pred(ty) {
                        continue;
                    }
                    let mut recs: Vec<&Record> = sec.records.iter().filter(|r| r.ty == ty).collect();
                    let n = recs.len();
                    recs.sort_by_key(|r| (java_bucket(r.pos, n), r.seq));
                    out.extend(recs.into_iter().filter(|r| q.keeps(r)).copied());
                }
            }
        }
        out
    }
}

#[cfg(test)]
impl Pois {
    /// Tests: the registry's saveable state in a fixed order (sections with their records and
    /// insertion ranks, indexed chunks, chunks waiting to be saved).
    pub(crate) fn fingerprint(&self) -> String {
        let mut keys: Vec<_> = self.sections.keys().copied().collect();
        keys.sort_unstable();
        let mut out = String::new();
        for k in keys {
            let s = &self.sections[&k];
            out += &format!("{k:?} valid {} {:?}\n", s.valid, s.records);
        }
        let mut indexed: Vec<_> = self.indexed.iter().copied().collect();
        indexed.sort_unstable();
        let mut dirty: Vec<_> = self.dirty.iter().copied().collect();
        dirty.sort_unstable();
        out + &format!("indexed {indexed:?}\ndirty {dirty:?}\nseq {}\n", self.seq)
    }
}

/// Columns at exactly horizontal Chebyshev distance `ring` from `center` (the border of a square).
fn ring_columns(center: (i32, i32), ring: i32) -> impl Iterator<Item = (i32, i32)> {
    let (x, z) = center;
    let side = -ring..=ring;
    // Rows z = ±ring whole, then the columns x = ±ring between them (none for ring 0).
    let rows = [-ring, ring].into_iter().take(if ring == 0 { 1 } else { 2 });
    let ends = rows.flat_map(move |dz| side.clone().map(move |dx| (x + dx, z + dz)));
    let sides = (-ring + 1..ring).flat_map(move |dz| [(x - ring, z + dz), (x + ring, z + dz)]);
    ends.chain(sides)
}

fn chebyshev(a: (i32, i32, i32), b: (i32, i32, i32)) -> i32 {
    (a.0 - b.0).abs().max((a.1 - b.1).abs()).max((a.2 - b.2).abs())
}

/// `sections_to_village(s)` read from a collection of village sections that holds every one
/// within 6 sections of `s` (`Pois::village_sections_in_cube`).
pub(crate) fn village_distance_among(villages: &[(i32, i32, i32)], s: (i32, i32, i32)) -> i32 {
    villages.iter().map(|&v| chebyshev(v, s)).min().map_or(VILLAGE_FAR, |d| d.min(VILLAGE_FAR))
}

impl Server {
    fn poi_table(&mut self) {
        if !self.mobs.poi.table.is_empty() {
            return;
        }
        let n = self.data.states.len();
        let mut t = vec![0u8; n];
        for (s, slot) in t.iter_mut().enumerate() {
            let s = s as u32;
            let Some(b) = self.data.block_of(s) else { continue };
            let props: Vec<(&str, &str)> = self.data.state_properties(s).collect();
            if let Some(ty) = type_of_block(&b.name, &props) {
                *slot = ty as u8 + 1;
            }
        }
        self.mobs.poi.table = t;
    }

    /// POI type of a block state.
    pub(crate) fn poi_type_of_state(&mut self, s: u32) -> Option<PoiType> {
        self.poi_table();
        let v = *self.mobs.poi.table.get(s as usize)?;
        (v > 0).then(|| ALL[(v - 1) as usize])
    }

    /// State change of a block (called on write): deferred to the next tick if the POI type
    /// changes.
    pub(crate) fn poi_block_changed(&mut self, p: [i32; 3], old: u32, new: u32) {
        if self.mobs.poi.table.is_empty() && self.mobs.poi.indexed.is_empty() {
            // Registry never used: the chunk scan will see it.
            return;
        }
        let (a, b) = (self.poi_type_of_state(old), self.poi_type_of_state(new));
        if a != b && self.mobs.poi.indexed.contains(&(p[0] >> 4, p[2] >> 4)) {
            self.mobs.poi.pending.push((p, a, b));
        }
    }

    /// Applies the deferred changes (removal then addition, in the order of the writes).
    pub(crate) fn poi_flush(&mut self) {
        let pending = std::mem::take(&mut self.mobs.poi.pending);
        for (p, old, new) in pending {
            if old.is_some() {
                self.mobs.poi.remove_record(p);
            }
            if let Some(t) = new {
                let max = t.max_tickets();
                self.mobs.poi.add_record(p, t, max);
            }
        }
    }

    /// Sections of `poi/` read with a chunk (`Sections` → `Records`).
    pub(crate) fn poi_loaded(&mut self, list: Vec<((i32, i32), Option<Nbt>)>) {
        for (c, nbt) in list {
            let Some(root) = nbt else { continue };
            let Some(Nbt::Compound(secs)) = root.get("Sections") else { continue };
            let mut out = Vec::new();
            for (y, sec) in secs {
                let Ok(sy) = y.parse::<i32>() else { continue };
                let valid = crate::nbtio::int(sec, "Valid").is_some_and(|v| v != 0);
                let mut records = Vec::new();
                for r in crate::nbtio::list(sec.get("Records")) {
                    let Some(Nbt::IntArray(p)) = r.get("pos") else { continue };
                    if p.len() != 3 {
                        continue;
                    }
                    let Some(ty) = r.get("type").and_then(Nbt::as_str).and_then(PoiType::from_name) else { continue };
                    let free = crate::nbtio::int(r, "free_tickets").unwrap_or(0) as i32;
                    self.mobs.poi.seq += 1;
                    records.push(Record { pos: [p[0], p[1], p[2]], ty, free, seq: self.mobs.poi.seq });
                }
                out.push(((c.0, sy, c.1), Section::new(records, valid)));
            }
            self.mobs.poi.from_disk.insert(c, out);
        }
    }

    /// Indexes a loaded chunk: sections read from `poi/` kept if valid, otherwise scanned
    /// (tickets kept for the positions already known). True if the chunk is indexed afterwards
    /// (false: not loaded).
    pub(crate) fn poi_ensure(&mut self, cx: i32, cz: i32) -> bool {
        if self.mobs.poi.indexed.contains(&(cx, cz)) {
            return true;
        }
        if !self.chunks.get(&(cx, cz)).is_some_and(Option::is_some) {
            return false;
        }
        self.poi_table();
        self.mobs.poi.indexed.insert((cx, cz));
        let mut disk: HashMap<i32, Section> =
            self.mobs.poi.from_disk.remove(&(cx, cz)).unwrap_or_default().into_iter().map(|(k, s)| (k.1, s)).collect();
        let min_sec = self.dim.height.min_y >> 4;
        let n = self.dim.height.sections as i32;
        let mut changed = false;
        for si in 0..n {
            let sy = min_sec + si;
            let key = (cx, sy, cz);
            let mut old = disk.remove(&sy);
            if let Some(s) = old.take_if(|s| s.valid) {
                self.mobs.poi.insert_section(key, s);
                continue;
            }
            // Scan of the section's blocks (palette first).
            let found = self.poi_scan_section(cx, si as usize, sy, cz);
            if found.is_empty() && old.is_none() {
                continue;
            }
            let mut records = Vec::with_capacity(found.len());
            for (p, ty) in found {
                let free = old
                    .as_ref()
                    .and_then(|o| o.records.iter().find(|r| r.pos == p && r.ty == ty))
                    .map_or_else(|| ty.max_tickets(), |r| r.free);
                self.mobs.poi.seq += 1;
                records.push(Record { pos: p, ty, free, seq: self.mobs.poi.seq });
            }
            self.mobs.poi.insert_section(key, Section::new(records, true));
            changed = true;
        }
        if changed {
            self.mobs.poi.dirty.insert((cx, cz));
        }
        true
    }

    /// Indexes the square of chunks of `radius` around chunk (`cx`, `cz`), rows of z, x fastest
    /// (the order sets the insertion ranks). A square once found fully indexed stays so: it is
    /// then skipped in one lookup instead of (2 radius + 1)² (`RUSTCRAFT_OPT_POIINDEXEDAREAS`).
    pub(crate) fn poi_ensure_square(&mut self, cx: i32, cz: i32, radius: i32) {
        let memo = self.mobs.poi.paths.indexed_areas;
        if memo && self.mobs.poi.indexed_squares.contains(&(cx, cz, radius)) {
            note_search_path("indexed square skipped");
            return;
        }
        let mut all_indexed = true;
        for z in cz - radius..=cz + radius {
            for x in cx - radius..=cx + radius {
                all_indexed &= self.poi_ensure(x, z);
            }
        }
        if memo && all_indexed {
            self.mobs.poi.indexed_squares.insert((cx, cz, radius));
        }
    }

    fn poi_scan_section(&self, cx: i32, si: usize, sy: i32, cz: i32) -> Vec<([i32; 3], PoiType)> {
        let mut out = Vec::new();
        let Some(Some(c)) = self.chunks.get(&(cx, cz)) else { return out };
        let Some(sec) = c.sections.get(si) else { return out };
        let t = &self.mobs.poi.table;
        let has = |s: u32| t.get(s as usize).is_some_and(|&v| v > 0);
        let maybe = match &sec.blocks {
            rc_world::Container::Single(s) => has(*s),
            rc_world::Container::Packed { palette: Some(p), .. } => p.iter().any(|&s| has(s)),
            rc_world::Container::Packed { palette: None, .. } => true,
        };
        if !maybe {
            return out;
        }
        // Game order: the contained blocks (x, then z, then y the slowest).
        for y in 0..16 {
            for z in 0..16 {
                for x in 0..16 {
                    let s = sec.blocks.get(rc_world::block_index(x, y, z));
                    if let Some(&v) = t.get(s as usize)
                        && v > 0
                    {
                        out.push(([cx * 16 + x as i32, sy * 16 + y as i32, cz * 16 + z as i32], ALL[(v - 1) as usize]));
                    }
                }
            }
        }
        out
    }

    fn poi_ensure_around(&mut self, center: [i32; 3], radius: i32) {
        let cr = radius.div_euclid(16) + 1;
        self.poi_ensure_square(center[0] >> 4, center[2] >> 4, cr);
    }

    /// Chunks (x then z), sections from bottom to top, order of the game's sets;
    /// square then disc (distance² to the centre of the origin block ≤ r²).
    pub(crate) fn poi_in_range(
        &mut self,
        pred: impl Fn(PoiType) -> bool,
        center: [i32; 3],
        radius: i32,
        occ: Occupancy,
    ) -> Vec<Record> {
        self.poi_flush();
        self.poi_ensure_around(center, radius);
        let min_sec = self.dim.height.min_y >> 4;
        let heights = min_sec..=min_sec + self.dim.height.sections as i32 - 1;
        let query = RangeQuery { center, radius, occ, heights };
        self.mobs.poi.records_in_range(pred, &query)
    }

    /// First minimum of distance² in the order of the elements in range.
    pub(crate) fn poi_closest(
        &mut self,
        pred: impl Fn(PoiType) -> bool,
        center: [i32; 3],
        radius: i32,
        occ: Occupancy,
    ) -> Option<[i32; 3]> {
        let list = self.poi_in_range(pred, center, radius, occ);
        let mut best: Option<(i64, [i32; 3])> = None;
        for r in list {
            let d = dist2i(r.pos, center);
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, r.pos));
            }
        }
        best.map(|b| b.1)
    }

    pub(crate) fn poi_type(&mut self, p: [i32; 3]) -> Option<PoiType> {
        self.poi_flush();
        self.poi_ensure(p[0] >> 4, p[2] >> 4);
        self.mobs.poi.get_type(p)
    }

    pub(crate) fn poi_release(&mut self, p: [i32; 3]) -> bool {
        self.poi_flush();
        self.poi_ensure(p[0] >> 4, p[2] >> 4);
        self.mobs.poi.release(p)
    }

    pub(crate) fn poi_acquire(&mut self, p: [i32; 3]) -> bool {
        self.poi_flush();
        self.poi_ensure(p[0] >> 4, p[2] >> 4);
        self.mobs.poi.acquire(p)
    }

    /// Pending changes applied, then the chunks a village distance test of section column
    /// (`cx`, `cz`) reads indexed (±7 around it).
    fn poi_prepare_village_test(&mut self, cx: i32, cz: i32) {
        self.poi_flush();
        self.poi_ensure_square(cx, cz, VILLAGE_INDEX_RADIUS);
    }

    /// The distance in sections to the village of a position (16³ sections).
    pub(crate) fn sections_to_village(&mut self, p: [i32; 3]) -> i32 {
        let s = section_key(p);
        self.poi_prepare_village_test(s.0, s.2);
        self.mobs.poi.sections_to_village(s)
    }

    /// `sections_to_village(p) <= d`, reading only what the bound needs (same indexing).
    pub(crate) fn within_sections_of_village(&mut self, p: [i32; 3], d: i32) -> bool {
        let s = section_key(p);
        self.poi_prepare_village_test(s.0, s.2);
        self.mobs.poi.within_sections_of_village(s, d)
    }

    /// At most one section from a village centre.
    pub(crate) fn is_village(&mut self, p: [i32; 3]) -> bool {
        self.within_sections_of_village(p, 1)
    }

    /// Prepares a batch of village tests at once: pending changes applied, then, for each
    /// section column of `columns` in the order given, the chunks one `sections_to_village` call
    /// would index. A caller lists its columns in the order of its per-section calls (each
    /// column once), so the chunks are indexed in the same order (`RUSTCRAFT_OPT_VILLAGEBATCH`).
    pub(crate) fn poi_prepare_village_batch(&mut self, columns: impl IntoIterator<Item = (i32, i32)>) {
        note_search_path("village batch");
        self.poi_flush();
        for (cx, cz) in columns {
            self.poi_ensure_square(cx, cz, VILLAGE_INDEX_RADIUS);
        }
    }

    /// Chunks of `poi/` to rewrite.
    pub(crate) fn poi_save_jobs(&mut self, jobs: &mut Vec<crate::save::Job>) {
        self.poi_flush();
        // Generator of the wandering trader (same family).
        self.trader_save_job(jobs);
        let dirty: Vec<(i32, i32)> = self.mobs.poi.dirty.drain().collect();
        let min_sec = self.dim.height.min_y >> 4;
        let n = self.dim.height.sections as i32;
        for (cx, cz) in dirty {
            let mut secs = Vec::new();
            for sy in min_sec..min_sec + n {
                let Some(s) = self.mobs.poi.sections.get(&(cx, sy, cz)) else { continue };
                let records: Vec<Nbt> = s
                    .records
                    .iter()
                    .map(|r| {
                        Nbt::Compound(vec![
                            ("pos".into(), Nbt::IntArray(r.pos.to_vec())),
                            ("type".into(), Nbt::String(r.ty.name().into())),
                            ("free_tickets".into(), Nbt::Int(r.free)),
                        ])
                    })
                    .collect();
                secs.push((
                    sy.to_string(),
                    Nbt::Compound(vec![
                        ("Valid".into(), Nbt::Byte(i8::from(s.valid))),
                        ("Records".into(), Nbt::List(records)),
                    ]),
                ));
            }
            let root = Nbt::Compound(vec![
                ("Sections".into(), Nbt::Compound(secs)),
                ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)),
            ]);
            let path = self.dim.dir.join("poi").join(crate::region::region_name(cx, cz));
            jobs.push(crate::save::Job::Chunk { path, cx, cz, nbt: crate::nbtio::encode_root(&root) });
        }
    }
}

pub fn dist2i(a: [i32; 3], b: [i32; 3]) -> i64 {
    let d = |i: usize| i64::from(a[i] - b[i]);
    d(0) * d(0) + d(1) * d(1) + d(2) * d(2)
}

#[cfg(test)]
#[path = "poi_tests.rs"]
mod search_path_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_types() {
        assert_eq!(type_of_block("minecraft:red_bed", &[("part", "head")]), Some(PoiType::Home));
        assert_eq!(type_of_block("minecraft:red_bed", &[("part", "foot")]), None);
        // The straw bed is not a villager home.
        assert_eq!(type_of_block("minecraft:straw_bed", &[("part", "head")]), None);
        assert_eq!(type_of_block("minecraft:water_cauldron", &[]), Some(PoiType::Leatherworker));
        assert_eq!(type_of_block("minecraft:waxed_exposed_lightning_rod", &[]), Some(PoiType::LightningRod));
        assert_eq!(type_of_block("minecraft:bell", &[]), Some(PoiType::Meeting));
        assert!(PoiType::Weaponsmith.is_job() && !PoiType::Home.is_job() && PoiType::Meeting.is_village());
        assert_eq!(PoiType::Meeting.max_tickets(), 32);
    }

    #[test]
    fn village_distance() {
        let mut p = Pois::default();
        p.add_record([5, 70, 5], PoiType::Home, 1);
        assert_eq!(p.sections_to_village((0, 4, 0)), 7, "free bed: not a centre");
        assert!(p.acquire([5, 70, 5]));
        assert_eq!(p.sections_to_village((0, 4, 0)), 0);
        assert_eq!(p.sections_to_village((2, 4, -1)), 2);
        assert_eq!(p.sections_to_village((7, 4, 0)), 7);
        assert!(!p.acquire([5, 70, 5]), "no more tickets");
        assert!(p.release([5, 70, 5]));
        assert!(!p.release([5, 70, 5]), "capped at the maximum");
    }
}
