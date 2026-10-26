//! Points of interest (F05, `docs/specs/server-mobs.md` § "Generic POI mechanism"):
//! registry per chunk section (position, type, free tickets), updated on each
//! block state change (applied on the next tick, like the game's deferred task), read and
//! written in `poi/` in the game's format, searches within a radius and distance to the village.

use hashbrown::{HashMap, HashSet};

use rc_proto::nbt::Nbt;

use crate::server::Server;

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
    records: Vec<Record>,
    valid: bool,
}

/// State change pending: position, old type, new type.
type Change = ([i32; 3], Option<PoiType>, Option<PoiType>);

/// Section read from `poi/`: section (x, y, z) key.
type DiskSection = ((i32, i32, i32), Section);

/// POI registry of a dimension.
#[derive(Debug, Default)]
pub struct Pois {
    /// Type per state identifier (0 = none, otherwise type + 1), built on first use.
    table: Vec<u8>,
    sections: HashMap<(i32, i32, i32), Section>,
    /// Indexed chunks (read from disk or scanned).
    indexed: HashSet<(i32, i32)>,
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
    fn add_record(&mut self, p: [i32; 3], ty: PoiType, free: i32) -> bool {
        self.seq += 1;
        let seq = self.seq;
        let s = self.sections.entry(section_key(p)).or_insert_with(|| Section { records: Vec::new(), valid: true });
        if let Some(r) = s.records.iter().find(|r| r.pos == p) {
            if r.ty == ty {
                return false;
            }
            s.records.retain(|r| r.pos != p);
        }
        s.records.push(Record { pos: p, ty, free, seq });
        self.dirty.insert((p[0] >> 4, p[2] >> 4));
        true
    }

    fn remove_record(&mut self, p: [i32; 3]) {
        if let Some(s) = self.sections.get_mut(&section_key(p)) {
            let before = s.records.len();
            s.records.retain(|r| r.pos != p);
            if s.records.len() != before {
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
        let ok = match self.record_mut(p) {
            Some(r) if r.free < r.ty.max_tickets() => {
                r.free += 1;
                true
            }
            _ => false,
        };
        self.dirty.insert((p[0] >> 4, p[2] >> 4));
        ok
    }

    /// Takes a ticket; false if none remain.
    pub fn acquire(&mut self, p: [i32; 3]) -> bool {
        let ok = match self.record_mut(p) {
            Some(r) if r.free > 0 => {
                r.free -= 1;
                true
            }
            _ => false,
        };
        if ok {
            self.dirty.insert((p[0] >> 4, p[2] >> 4));
        }
        ok
    }

    /// Distance (in sections) to the nearest village centre, 7 beyond 6.
    pub fn sections_to_village(&self, s: (i32, i32, i32)) -> i32 {
        let mut best = 7;
        for dx in -6..=6 {
            for dy in -6..=6 {
                for dz in -6..=6 {
                    let d = i32::max(i32::abs(dx), i32::max(i32::abs(dy), i32::abs(dz)));
                    if d >= best {
                        continue;
                    }
                    if let Some(sec) = self.sections.get(&(s.0 + dx, s.1 + dy, s.2 + dz))
                        && sec.records.iter().any(|r| r.ty.is_village() && r.occupied())
                    {
                        best = d;
                    }
                }
            }
        }
        best
    }
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
                out.push(((c.0, sy, c.1), Section { records, valid }));
            }
            self.mobs.poi.from_disk.insert(c, out);
        }
    }

    /// Indexes a loaded chunk: sections read from `poi/` kept if valid, otherwise scanned
    /// (tickets kept for the positions already known).
    pub(crate) fn poi_ensure(&mut self, cx: i32, cz: i32) {
        if self.mobs.poi.indexed.contains(&(cx, cz)) {
            return;
        }
        if !self.chunks.get(&(cx, cz)).is_some_and(Option::is_some) {
            return;
        }
        self.poi_table();
        self.mobs.poi.indexed.insert((cx, cz));
        let disk: HashMap<i32, Section> =
            self.mobs.poi.from_disk.remove(&(cx, cz)).unwrap_or_default().into_iter().map(|(k, s)| (k.1, s)).collect();
        let min_sec = self.dim.height.min_y >> 4;
        let n = self.dim.height.sections as i32;
        let mut changed = false;
        for si in 0..n {
            let sy = min_sec + si;
            let key = (cx, sy, cz);
            let old = disk.get(&sy).cloned();
            if let Some(s) = &old
                && s.valid
            {
                self.mobs.poi.sections.insert(key, s.clone());
                continue;
            }
            // Scan of the section's blocks (palette first).
            let found = self.poi_scan_section(cx, si as usize, sy, cz);
            if found.is_empty() && old.is_none() {
                continue;
            }
            let mut sec = Section { records: Vec::new(), valid: true };
            for (p, ty) in found {
                let free = old
                    .as_ref()
                    .and_then(|o| o.records.iter().find(|r| r.pos == p && r.ty == ty))
                    .map_or_else(|| ty.max_tickets(), |r| r.free);
                self.mobs.poi.seq += 1;
                sec.records.push(Record { pos: p, ty, free, seq: self.mobs.poi.seq });
            }
            self.mobs.poi.sections.insert(key, sec);
            changed = true;
        }
        if changed {
            self.mobs.poi.dirty.insert((cx, cz));
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
        let (ccx, ccz) = (center[0] >> 4, center[2] >> 4);
        for cz in ccz - cr..=ccz + cr {
            for cx in ccx - cr..=ccx + cr {
                self.poi_ensure(cx, cz);
            }
        }
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
        let cr = radius.div_euclid(16) + 1;
        let (ccx, ccz) = (center[0] >> 4, center[2] >> 4);
        let min_sec = self.dim.height.min_y >> 4;
        let max_sec = min_sec + self.dim.height.sections as i32 - 1;
        let r2 = i64::from(radius) * i64::from(radius);
        let mut out = Vec::new();
        for cz in ccz - cr..=ccz + cr {
            for cx in ccx - cr..=ccx + cr {
                for sy in min_sec..=max_sec {
                    let Some(sec) = self.mobs.poi.sections.get(&(cx, sy, cz)) else { continue };
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
                        for r in recs {
                            if !occ.test(r) {
                                continue;
                            }
                            let (dx, dz) = (r.pos[0] - center[0], r.pos[2] - center[2]);
                            if dx.abs() > radius || dz.abs() > radius {
                                continue;
                            }
                            if dist2i(r.pos, center) <= r2 {
                                out.push(*r);
                            }
                        }
                    }
                }
            }
        }
        out
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

    /// The distance in sections to the village of a position (16³ sections).
    pub(crate) fn sections_to_village(&mut self, p: [i32; 3]) -> i32 {
        self.poi_flush();
        let s = section_key(p);
        for dz in -7..=7 {
            for dx in -7..=7 {
                self.poi_ensure(s.0 + dx, s.2 + dz);
            }
        }
        self.mobs.poi.sections_to_village(s)
    }

    /// At most one section from a village centre.
    pub(crate) fn is_village(&mut self, p: [i32; 3]) -> bool {
        self.sections_to_village(p) <= 1
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
