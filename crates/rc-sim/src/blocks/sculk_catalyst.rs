//! Sculk catalysts. A creature dying within 8 blocks makes the nearest catalyst bloom and take the
//! experience the creature would drop: it becomes charges that travel through the sculk around
//! the catalyst, turning the blocks they reach into sculk, laying veins, and growing sensors and
//! shriekers. The charges live in the catalyst's block entity (`cursors`), read and written back
//! by its tick only while there are some.

use rc_data::PropKey;
use rc_proto::nbt::Nbt;

use super::attach::AttachRules;
use super::{Dir, P, above, flag, rel};
use crate::server::Server;
use crate::tag_bits::TagId;

/// Highest charge one cursor holds; a larger amount is split.
const MAX_CHARGE: i32 = 1000;
/// Cursors one catalyst keeps at most (the others are dropped).
const MAX_CURSORS: usize = 32;
/// A catalyst hears the deaths within this many blocks.
const LISTENER_RADIUS: i32 = 8;
/// A cursor farther than this (chessboard distance) from its catalyst is left alone.
const MAX_CURSOR_DISTANCE: i32 = 1024;
/// Charge spent on a sensor or shrieker grown out of sculk.
const GROWTH_COST: i32 = 10;
/// No growth this close to the catalyst.
const NO_GROWTH_RADIUS: i32 = 4;
/// One chance in this many, per update, that a charge on sculk decays (or grows something).
const CHARGE_DECAY_RATE: i32 = 10;
/// One chance in this many that a charge that could not grow loses some.
const ADDITIONAL_DECAY_RATE: i32 = 5;
/// The charge growth draws a shrieker one time in this many (a sensor otherwise).
const SHRIEKER_ONE_IN: i32 = 11;
/// Ticks a catalyst stays in bloom.
const PULSE_TICKS: i32 = 8;

/// The 18 cells around a cell sharing a face or an edge with it, in the game's walk order
/// (x fastest, then y, then z).
const NEIGHBOURS_WITHOUT_CORNERS: [[i32; 3]; 18] = {
    let mut out = [[0; 3]; 18];
    let mut n = 0;
    let mut z = -1;
    while z <= 1 {
        let mut y = -1;
        while y <= 1 {
            let mut x = -1;
            while x <= 1 {
                if (x == 0 || y == 0 || z == 0) && !(x == 0 && y == 0 && z == 0) {
                    out[n] = [x, y, z];
                    n += 1;
                }
                x += 1;
            }
            y += 1;
        }
        z += 1;
    }
    out
};

/// Network names of the directions, in the game's order.
const DIRECTION_NAMES: [&str; 6] = ["down", "up", "north", "south", "west", "east"];

/// One charge travelling through the sculk (the game's cursor).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SculkCharge {
    pub pos: P,
    pub charge: i32,
    /// 1 while the charge may still decay on a block that is not sculk, 0 after.
    pub decay_delay: i32,
    /// Ticks to wait before the next update.
    pub update_delay: i32,
    /// Faces of the vein it last stood on (one bit per direction), `None` before it moved.
    pub facings: Option<u8>,
}

impl SculkCharge {
    fn new(pos: P, charge: i32) -> SculkCharge {
        SculkCharge { pos, charge, decay_delay: 1, update_delay: 0, facings: None }
    }

    fn to_nbt(&self) -> Nbt {
        let mut f = vec![
            ("pos".to_owned(), Nbt::IntArray(self.pos.to_vec())),
            ("charge".to_owned(), Nbt::Int(self.charge)),
            ("decay_delay".to_owned(), Nbt::Int(self.decay_delay)),
            ("update_delay".to_owned(), Nbt::Int(self.update_delay)),
        ];
        if let Some(bits) = self.facings {
            let names = Dir::ALL.iter().filter(|&&d| bits >> d as u8 & 1 != 0).map(|&d| Nbt::String(DIRECTION_NAMES[d as usize].into()));
            f.push(("facings".to_owned(), Nbt::List(names.collect())));
        }
        Nbt::Compound(f)
    }

    /// A cursor as the game reads it; `None` for a value out of its range (the whole list is
    /// then refused). Unknown directions make the facings unknown, like a lenient field.
    fn from_nbt(n: &Nbt) -> Option<SculkCharge> {
        let pos = match n.get("pos")? {
            Nbt::IntArray(v) if v.len() == 3 => [v[0], v[1], v[2]],
            _ => return None,
        };
        let int = |k: &str, default: i32| n.get(k).map_or(Some(default), |v| v.as_i64().map(|v| v as i32));
        let charge = int("charge", 0).filter(|c| (0..=MAX_CHARGE).contains(c))?;
        let decay_delay = int("decay_delay", 1).filter(|d| (0..=1).contains(d))?;
        let update_delay = int("update_delay", 0).filter(|d| *d >= 0)?;
        let facings = match n.get("facings") {
            Some(Nbt::List(l)) => l.iter().try_fold(0u8, |bits, v| {
                let i = DIRECTION_NAMES.iter().position(|d| Some(*d) == v.as_str())?;
                Some(bits | 1 << i)
            }),
            _ => None,
        };
        Some(SculkCharge { pos, charge, decay_delay, update_delay, facings })
    }
}

/// The cursors of a catalyst's block entity (at most 32; any invalid entry refuses the list).
fn cursors_of(be: &Nbt) -> Vec<SculkCharge> {
    match be.get("cursors") {
        Some(Nbt::List(l)) if l.len() <= MAX_CURSORS => l.iter().map(SculkCharge::from_nbt).collect::<Option<_>>().unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// What the sculk catalysts did with a death they heard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CatalystHeard {
    /// No catalyst around: the experience drops as usual.
    Nothing,
    /// The nearest catalyst bloomed and took the experience, with nothing to spread.
    Bloomed,
    /// The nearest catalyst bloomed and the experience became its charges.
    BloomedWithCharges,
}

impl CatalystHeard {
    /// A catalyst took the experience: no orb drops.
    pub(crate) fn took_experience(self) -> bool {
        self != CatalystHeard::Nothing
    }
}

/// What a block does with a charge on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SculkKind {
    /// Any other block: the charge only waits out its decay delay.
    Other,
    Sculk,
    Vein,
}

/// The faces of a vein as bits (empty for anything else).
fn face_bits(v: &super::table::View<'_>, s: u32) -> u8 {
    Dir::ALL.iter().filter(|&&d| v.get_bool(s, d.key())).fold(0, |bits, &d| bits | 1 << d as u8)
}

/// The game's shuffle (from the end, each slot swapped with one drawn in `[0, i]`) on the level's
/// random.
fn shuffle<T>(rng: &mut super::JavaRandom, v: &mut [T]) {
    for i in (1..v.len()).rev() {
        let j = rng.next_int_bound(i as i32 + 1) as usize;
        v.swap(i, j);
    }
}

fn dist_sqr(a: P, b: P) -> f64 {
    (0..3).map(|i| f64::from(a[i] - b[i]).powi(2)).sum()
}

impl Server {
    /// The state's fluid is still water (a water block's source or a waterlogged block), not
    /// flowing water.
    fn holds_water_source(&self, s: u32) -> bool {
        let f = self.fluid_of(s);
        f.kind == 1 && f.source
    }

    fn sculk_kind(&self, s: u32) -> SculkKind {
        let b = self.view().block(s);
        if b == self.bs.t.id.sculk {
            SculkKind::Sculk
        } else if b == self.bs.t.id.sculk_vein {
            SculkKind::Vein
        } else {
            SculkKind::Other
        }
    }

    // ---------- Hearing a death ----------

    /// A death at `at`: the nearest living catalyst within 8 blocks takes it. It always blooms;
    /// if the dead may drop experience (`may_drop`) and `reward` (drawn here, once) is positive,
    /// the experience becomes charges from just above the body. Once a catalyst took it, no orb
    /// drops.
    pub(crate) fn catalyst_takes_death(
        &mut self,
        at: [f64; 3],
        may_drop: bool,
        reward: impl FnOnce(&mut Server) -> i32,
    ) -> CatalystHeard {
        let Some(catalyst) = self.nearest_catalyst(at) else { return CatalystHeard::Nothing };
        let xp = reward(self);
        let charged = may_drop && xp > 0;
        if charged {
            let start = [at[0].floor() as i32, (at[1] + 0.5).floor() as i32, at[2].floor() as i32];
            self.catalyst_add_charges(catalyst, start, xp);
        }
        self.catalyst_bloom(catalyst);
        if charged { CatalystHeard::BloomedWithCharges } else { CatalystHeard::Bloomed }
    }

    /// The catalyst that hears a death at `at` first: the closest block centre within the
    /// radius (block distance), among the living catalysts of the dimension.
    fn nearest_catalyst(&self, at: [f64; 3]) -> Option<P> {
        let source = at.map(|v| v.floor() as i32);
        let r2 = f64::from(LISTENER_RADIUS * LISTENER_RADIUS);
        let centre_dist = |p: &P| (0..3).map(|i| (f64::from(p[i]) + 0.5 - at[i]).powi(2)).sum::<f64>();
        self.bs
            .be_ticks
            .iter()
            .filter(|(p, (_, t))| matches!(t, super::level::BeTick::Catalyst) && dist_sqr(**p, source) <= r2)
            .map(|(p, _)| *p)
            .filter(|&p| self.view().block(self.get(p)) == self.bs.t.id.sculk_catalyst && self.be_get(p).is_some())
            .min_by(|a, b| centre_dist(a).total_cmp(&centre_dist(b)).then(a.cmp(b)))
    }

    /// `amount` of experience added to the catalyst at `c` as charges from `start` (split by
    /// 1,000; past 32 cursors the rest is lost).
    fn catalyst_add_charges(&mut self, c: P, start: P, amount: i32) {
        let Some(be) = self.be_get(c) else { return };
        let mut cursors = cursors_of(be);
        let mut left = amount;
        while left > 0 {
            let part = left.min(MAX_CHARGE);
            if cursors.len() < MAX_CURSORS {
                cursors.push(SculkCharge::new(start, part));
            }
            left -= part;
        }
        self.store_cursors(c, &cursors);
    }

    fn store_cursors(&mut self, c: P, cursors: &[SculkCharge]) {
        if let Some(be) = self.be_get_mut(c) {
            crate::nbtio::set(be, "cursors", Nbt::List(cursors.iter().map(SculkCharge::to_nbt).collect()));
        }
        if let Some(Some(chunk)) = self.chunks.get_mut(&(c[0] >> 4, c[2] >> 4)) {
            chunk.dirty = true;
        }
    }

    /// Bloom: the `bloom` state for 8 ticks, sculk souls above it, its sound.
    fn catalyst_bloom(&mut self, c: P) {
        let s = self.get(c);
        let bloomed = self.view().set_named(s, "bloom", "true");
        self.set_update(c, bloomed);
        let b = self.view().block(bloomed);
        self.schedule_tick(c, b, PULSE_TICKS);
        let top = [f64::from(c[0]) + 0.5, f64::from(c[1]) + 1.15, f64::from(c[2]) + 0.5];
        self.particles("minecraft:sculk_soul", top, [0.2, 0.0, 0.2], 0.0, 2);
        let pitch = 0.6 + self.bs.rng.next_float() * 0.4;
        self.block_sound(c, "minecraft:block.sculk_catalyst.bloom", 2.0, pitch);
    }

    /// Scheduled tick of a catalyst: the bloom ends. False for any other block.
    pub(crate) fn catalyst_block_tick(&mut self, s: u32, p: P) -> bool {
        if self.view().block(s) != self.bs.t.id.sculk_catalyst {
            return false;
        }
        if self.view().get_named(s, "bloom") == Some("true") {
            let calm = self.view().set_named(s, "bloom", "false");
            self.set_update(p, calm);
        }
        true
    }

    // ---------- The charges move ----------

    /// Tick of a catalyst's block entity: its cursors move (nothing is read or written while it
    /// has none, the usual case).
    pub(crate) fn catalyst_tick(&mut self, c: P) {
        let has_cursors = self.be_get(c).is_some_and(|be| matches!(be.get("cursors"), Some(Nbt::List(l)) if !l.is_empty()));
        if !has_cursors {
            return;
        }
        let cursors = self.be_get(c).map(cursors_of).unwrap_or_default();
        let kept = self.update_cursors(c, cursors);
        self.store_cursors(c, &kept);
    }

    /// One update of every cursor of the catalyst at `origin`, then the merge of those that
    /// meet in a cell (up to 1,000 together) and the charge particles of each cell.
    fn update_cursors(&mut self, origin: P, cursors: Vec<SculkCharge>) -> Vec<SculkCharge> {
        let mut kept: Vec<SculkCharge> = Vec::with_capacity(cursors.len());
        // Per cell: the total charge, and the index in `kept` of the cursor others merge into.
        let mut cells: Vec<(P, i32, usize)> = Vec::new();
        for mut cursor in cursors {
            let far = (0..3).map(|i| (cursor.pos[i] - origin[i]).abs()).max().unwrap_or(0) > MAX_CURSOR_DISTANCE;
            if far {
                continue;
            }
            self.update_cursor(origin, &mut cursor);
            if cursor.charge <= 0 {
                self.level_event(3006, cursor.pos, 0);
                continue;
            }
            match cells.iter_mut().find(|(p, ..)| *p == cursor.pos) {
                None => {
                    cells.push((cursor.pos, cursor.charge, kept.len()));
                    kept.push(cursor);
                }
                Some((_, total, into)) => {
                    *total += cursor.charge;
                    let existing = &mut kept[*into];
                    if cursor.charge + existing.charge <= MAX_CHARGE {
                        existing.charge += cursor.charge;
                        existing.update_delay = existing.update_delay.min(cursor.update_delay);
                    } else {
                        let smaller = cursor.charge < existing.charge;
                        kept.push(cursor);
                        if smaller {
                            *into = kept.len() - 1;
                        }
                    }
                }
            }
        }
        for &(p, total, into) in &cells {
            if let Some(faces) = kept[into].facings.filter(|_| total > 0) {
                let particles = ((total as f64).ln_1p() / f64::from(2.3f32)) as i32 + 1;
                self.level_event(3006, p, (particles << 6) + i32::from(faces));
            }
        }
        kept
    }

    fn update_cursor(&mut self, origin: P, c: &mut SculkCharge) {
        if c.charge <= 0 {
            return;
        }
        if c.update_delay > 0 {
            c.update_delay -= 1;
            return;
        }
        let mut state = self.get(c.pos);
        let mut kind = self.sculk_kind(state);
        if self.attempt_spread_vein(kind, c.pos, state, c.facings) {
            // Sculk keeps its block when veins spread from it; the others are read again.
            if kind != SculkKind::Sculk {
                state = self.get(c.pos);
                kind = self.sculk_kind(state);
            }
            self.block_sound(c.pos, "minecraft:block.sculk.spread", 1.0, 1.0);
        }
        c.charge = self.use_charge(kind, c, origin);
        if c.charge <= 0 {
            self.on_discharged(kind, state, c.pos);
            return;
        }
        if let Some(to) = self.valid_movement_pos(c.pos) {
            self.on_discharged(kind, state, c.pos);
            c.pos = to;
            state = self.get(to);
        }
        if self.sculk_kind(state) != SculkKind::Other {
            c.facings = Some(face_bits(&self.view(), state));
        }
        c.decay_delay = if kind == SculkKind::Other { (c.decay_delay - 1).max(0) } else { 1 };
        c.update_delay = 1;
    }

    /// The charge left after its use on the block it stands on.
    fn use_charge(&mut self, kind: SculkKind, c: &SculkCharge, origin: P) -> i32 {
        match kind {
            SculkKind::Other => {
                if c.decay_delay > 0 {
                    c.charge
                } else {
                    0
                }
            }
            SculkKind::Vein => {
                if self.attempt_place_sculk(c.pos) {
                    c.charge - 1
                } else if self.bs.rng.next_int_bound(CHARGE_DECAY_RATE) == 0 {
                    (c.charge as f32 * 0.5).floor() as i32
                } else {
                    c.charge
                }
            }
            SculkKind::Sculk => self.sculk_use_charge(c, origin),
        }
    }

    /// A charge on sculk: now and then it grows a sensor or a shrieker above (far enough from
    /// the catalyst, without too many of them around) or decays, faster far from the catalyst.
    fn sculk_use_charge(&mut self, c: &SculkCharge, origin: P) -> i32 {
        let charge = c.charge;
        if charge == 0 || self.bs.rng.next_int_bound(CHARGE_DECAY_RATE) != 0 {
            return charge;
        }
        let close = dist_sqr(c.pos, origin) < f64::from(NO_GROWTH_RADIUS * NO_GROWTH_RADIUS);
        if !close && self.can_place_growth(c.pos) {
            if self.bs.rng.next_int_bound(GROWTH_COST) < charge {
                self.place_growth(c.pos);
            }
            return (charge - GROWTH_COST).max(0);
        }
        if self.bs.rng.next_int_bound(ADDITIONAL_DECAY_RATE) != 0 {
            return charge;
        }
        let penalty = if close {
            1
        } else {
            let outer = ((dist_sqr(c.pos, origin).sqrt() as f32) - NO_GROWTH_RADIUS as f32).powi(2);
            let reach = ((24 - NO_GROWTH_RADIUS) * (24 - NO_GROWTH_RADIUS)) as f32;
            ((charge as f32 * (outer / reach).min(1.0) * 0.5) as i32).max(1)
        };
        charge - penalty
    }

    /// Air (or water) above, and at most 2 growth inhibitors (sensors, shriekers) in the 9×3×9
    /// box from the cell up.
    fn can_place_growth(&self, p: P) -> bool {
        let up = self.get(above(p));
        let v = self.view();
        if !(v.is_air(up) || (v.block(up) == self.bs.t.id.water && self.holds_water_source(up))) {
            return false;
        }
        let mut found = 0;
        for x in p[0] - 4..=p[0] + 4 {
            for y in p[1]..=p[1] + 2 {
                for z in p[2] - 4..=p[2] + 4 {
                    if self.tag_id(TagId::SculkGrowthInhibitors, v.block(self.get([x, y, z]))) {
                        found += 1;
                        if found > 2 {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }

    /// A sensor (a shrieker one time in 11, unable to summon) grown above `p`, waterlogged in
    /// water, with its placement sound.
    fn place_growth(&mut self, p: P) {
        let at = above(p);
        let shrieker = self.bs.rng.next_int_bound(SHRIEKER_ONE_IN) == 0;
        let v = self.view();
        let mut s = if shrieker {
            let st = self.default_state("minecraft:sculk_shrieker");
            v.set_bool(st, PropKey::CanSummon, false)
        } else {
            self.default_state("minecraft:sculk_sensor")
        };
        if v.has(s, PropKey::Waterlogged) && !self.fluid_at(at).is_empty() {
            s = v.set_bool(s, PropKey::Waterlogged, true);
        }
        self.set_update(at, s);
        let sound = self.data.state(s).and_then(|i| self.data.sound_types.get(usize::from(i.sound))).map(|t| t.place.clone());
        if let Some(sound) = sound {
            self.block_sound(p, &sound, 1.0, 1.0);
        }
    }

    // ---------- Veins ----------

    /// Veins spread from the charge's block: from a block that is not sculk, those it already
    /// had face by face (`facings`), or all around it within the same cell before it ever moved.
    fn attempt_spread_vein(&mut self, kind: SculkKind, p: P, state: u32, facings: Option<u8>) -> bool {
        match (kind, facings) {
            (SculkKind::Other, None) => {
                let current = self.get(p);
                self.vein_spread_all(current, p, true) > 0
            }
            (SculkKind::Other, Some(faces)) if faces != 0 => {
                (self.view().is_air(state) || self.holds_water_source(state)) && self.vein_regrow(p, state, faces)
            }
            _ => self.vein_spread_all(state, p, false) > 0,
        }
    }

    /// A vein with the faces of `faces` that can attach placed at `p` (waterlogged in a fluid).
    fn vein_regrow(&mut self, p: P, existing: u32, faces: u8) -> bool {
        let v = self.view();
        let mut s = v.default_state(self.bs.t.id.sculk_vein);
        let mut any = false;
        for d in Dir::ALL {
            if faces >> d as u8 & 1 != 0 && self.multiface_can_attach(p, d) {
                s = v.set_bool(s, d.key(), true);
                any = true;
            }
        }
        if !any {
            return false;
        }
        if !self.fluid_of(existing).is_empty() {
            s = v.set_bool(s, PropKey::Waterlogged, true);
        }
        self.set_update(p, s);
        true
    }

    /// Spreading of veins from every face of `state` at `p` towards every direction (each face
    /// a source for a block that is not a vein); `same_cell`: only inside the cell itself. The
    /// number of faces placed.
    fn vein_spread_all(&mut self, state: u32, p: P, same_cell: bool) -> u32 {
        let is_vein = self.view().block(state) == self.bs.t.id.sculk_vein;
        let mut placed = 0;
        for from in Dir::ALL {
            if is_vein && !self.view().get_bool(state, from.key()) {
                continue;
            }
            for towards in Dir::ALL {
                if let Some((q, face)) = self.vein_spread_target(state, p, from, towards, same_cell)
                    && self.vein_spread_to(q, face)
                {
                    placed += 1;
                }
            }
        }
        placed
    }

    /// Where a vein spreads from face `from` of `state` at `p` towards `towards`: the same cell,
    /// then the neighbour on that side, then around the edge (only the first with `same_cell`).
    fn vein_spread_target(&self, state: u32, p: P, from: Dir, towards: Dir, same_cell: bool) -> Option<(P, Dir)> {
        if towards.axis() == from.axis() {
            return None;
        }
        let v = self.view();
        let other_source = v.block(state) != self.bs.t.id.sculk_vein;
        if !(other_source || (v.get_bool(state, from.key()) && !v.get_bool(state, towards.key()))) {
            return None;
        }
        let candidates = [(p, towards), (rel(p, towards), from), (rel(rel(p, towards), from), towards.opposite())];
        let n = if same_cell { 1 } else { 3 };
        candidates[..n].iter().copied().find(|&(q, face)| {
            let existing = self.get(q);
            self.vein_replaceable(p, q, face, existing) && self.vein_valid_placement(existing, q, face)
        })
    }

    /// The cell `q` may take a vein face `face` spread from `source`.
    fn vein_replaceable(&self, source: P, q: P, face: Dir, existing: u32) -> bool {
        let v = self.view();
        let t = &self.bs.t.id;
        let against = v.block(self.get(rel(q, face)));
        if against == t.sculk || against == t.sculk_catalyst || v.kind(self.get(rel(q, face))) == super::Kind::MovingPiston {
            return false;
        }
        let manhattan: i32 = (0..3).map(|i| (source[i] - q[i]).abs()).sum();
        if manhattan == 2 {
            let n = rel(source, face.opposite());
            if v.sturdy(self.get(n), face) {
                return false;
            }
        }
        let fluid = self.fluid_of(existing);
        // Any fluid but still water refuses (lava, flowing water).
        if !fluid.is_empty() && !self.holds_water_source(existing) {
            return false;
        }
        if self.tag_id(TagId::Fire, v.block(existing)) {
            return false;
        }
        if v.replaceable(existing) {
            return true;
        }
        v.is_air(existing) || v.block(existing) == t.sculk_vein || (v.block(existing) == t.water && fluid.source)
    }

    /// The face is not already there and can attach to the block behind it.
    fn vein_valid_placement(&self, existing: u32, q: P, face: Dir) -> bool {
        let v = self.view();
        if v.block(existing) == self.bs.t.id.sculk_vein && v.get_bool(existing, face.key()) {
            return false;
        }
        self.multiface_can_attach(q, face)
    }

    /// The vein face placed (flags 2), joined to a vein already there or waterlogged in a source.
    fn vein_spread_to(&mut self, q: P, face: Dir) -> bool {
        let old = self.get(q);
        if !self.vein_valid_placement(old, q, face) {
            return false;
        }
        let v = self.view();
        let vein = self.bs.t.id.sculk_vein;
        let base = if v.block(old) == vein {
            old
        } else if self.holds_water_source(old) {
            v.set_bool(v.default_state(vein), PropKey::Waterlogged, true)
        } else {
            v.default_state(vein)
        };
        self.set_block(q, v.set_bool(base, face.key(), true), flag::CLIENTS)
    }

    /// A vein turns the block it lies on into sculk (a face drawn at random among its own): the
    /// block becomes sculk, entities on it are pushed up, veins spread from it and the veins
    /// around it give up their faces against sculk.
    fn attempt_place_sculk(&mut self, p: P) -> bool {
        let state = self.get(p);
        let mut dirs = Dir::ALL;
        shuffle(&mut self.bs.rng, &mut dirs);
        for support in dirs {
            if !self.view().get_bool(state, support.key()) {
                continue;
            }
            let sp = rel(p, support);
            let old = self.get(sp);
            if !self.tag_id(TagId::SculkReplaceable, self.view().block(old)) {
                continue;
            }
            let sculk = self.view().default_state(self.bs.t.id.sculk);
            self.set_update(sp, sculk);
            // What stands on a lower block (mud, soul sand) rises onto the full sculk cube.
            let old_top = self.data.shape(self.view().info(old).collision).iter().map(|b| b.max[1]).fold(0.0, f64::max);
            self.push_entities_up(sp, old_top, 1.0);
            self.block_sound(sp, "minecraft:block.sculk.spread", 1.0, 1.0);
            self.vein_spread_all(sculk, sp, false);
            for d in Dir::ALL {
                if d == support.opposite() {
                    continue;
                }
                let vp = rel(sp, d);
                let vs = self.get(vp);
                if self.view().block(vs) == self.bs.t.id.sculk_vein {
                    self.on_discharged(SculkKind::Vein, vs, vp);
                }
            }
            return true;
        }
        false
    }

    /// A charge leaves a vein: its faces against sculk go, a vein left without faces becomes
    /// air or water.
    fn on_discharged(&mut self, kind: SculkKind, state: u32, p: P) {
        let v = self.view();
        if kind != SculkKind::Vein || v.block(state) != self.bs.t.id.sculk_vein {
            return;
        }
        let mut s = state;
        for d in Dir::ALL {
            if v.get_bool(s, d.key()) && v.block(self.get(rel(p, d))) == self.bs.t.id.sculk {
                s = v.set_bool(s, d.key(), false);
            }
        }
        if face_bits(&v, s) == 0 {
            s = if self.fluid_at(p).is_empty() { self.air() } else { v.default_state(self.bs.t.id.water) };
        }
        self.set_update(p, s);
    }

    // ---------- Where a charge goes next ----------

    /// The cell a charge moves to: one of the 18 neighbours without corners, drawn in a random
    /// order, holding sculk or a vein and reachable (an edge move needs one open side); the
    /// last such found, or the first vein lying on a block sculk can replace.
    fn valid_movement_pos(&mut self, p: P) -> Option<P> {
        let mut offsets = NEIGHBOURS_WITHOUT_CORNERS;
        shuffle(&mut self.bs.rng, &mut offsets);
        let mut best = p;
        for o in offsets {
            let n = [p[0] + o[0], p[1] + o[1], p[2] + o[2]];
            let s = self.get(n);
            if self.sculk_kind(s) != SculkKind::Other && self.movement_unobstructed(p, n) {
                best = n;
                if self.vein_has_substrate(s, n) {
                    break;
                }
            }
        }
        (best != p).then_some(best)
    }

    fn movement_unobstructed(&self, from: P, to: P) -> bool {
        let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        if d[0].abs() + d[1].abs() + d[2].abs() == 1 {
            return true;
        }
        let open = |dir: Dir| {
            let t = rel(from, dir);
            !self.view().sturdy(self.get(t), dir.opposite())
        };
        let dx = if d[0] < 0 { Dir::West } else { Dir::East };
        let dy = if d[1] < 0 { Dir::Down } else { Dir::Up };
        let dz = if d[2] < 0 { Dir::North } else { Dir::South };
        if d[0] == 0 {
            open(dy) || open(dz)
        } else if d[1] == 0 {
            open(dx) || open(dz)
        } else {
            open(dx) || open(dy)
        }
    }

    /// A vein with a face on a block sculk can replace.
    fn vein_has_substrate(&self, s: u32, p: P) -> bool {
        let v = self.view();
        v.block(s) == self.bs.t.id.sculk_vein
            && Dir::ALL.iter().any(|&d| v.get_bool(s, d.key()) && self.tag_id(TagId::SculkReplaceable, v.block(self.get(rel(p, d)))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cursors written and read back as the game stores them; an out-of-range value refuses
    /// the whole list.
    #[test]
    fn cursors_round_trip_and_refuse_bad_values() {
        let c = SculkCharge { pos: [1, -2, 3], charge: 40, decay_delay: 0, update_delay: 1, facings: Some(0b10_0001) };
        let back = SculkCharge::from_nbt(&c.to_nbt()).expect("read back");
        assert_eq!(back, c);
        let be = Nbt::Compound(vec![("cursors".into(), Nbt::List(vec![c.to_nbt()]))]);
        assert_eq!(cursors_of(&be), vec![c.clone()]);
        let mut bad = c.to_nbt();
        crate::nbtio::set(&mut bad, "charge", Nbt::Int(1001));
        let be = Nbt::Compound(vec![("cursors".into(), Nbt::List(vec![c.to_nbt(), bad]))]);
        assert!(cursors_of(&be).is_empty(), "one charge above 1,000: no cursor at all");
    }

    /// The 18 neighbours without corners, x fastest.
    #[test]
    fn neighbours_without_corners_in_walk_order() {
        assert_eq!(NEIGHBOURS_WITHOUT_CORNERS[0], [0, -1, -1]);
        assert_eq!(NEIGHBOURS_WITHOUT_CORNERS[1], [-1, 0, -1]);
        assert_eq!(NEIGHBOURS_WITHOUT_CORNERS[17], [0, 1, 1]);
        assert!(!NEIGHBOURS_WITHOUT_CORNERS.contains(&[1, 1, 1]) && !NEIGHBOURS_WITHOUT_CORNERS.contains(&[0, 0, 0]));
    }
}
