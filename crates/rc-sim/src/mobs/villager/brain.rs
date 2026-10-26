//! Villager brain (F05, spec § 3.3 and § 4.4): memories, sensors, activities and
//! behaviors ordered by priority. One tick: memories aged, sensors, start of the
//! stopped behaviors of the active activities, then tick or stop of those running.

use rc_proto::nbt::Nbt;

use super::super::jrand::JRand;
use super::super::mob::{Ent, Mob};
use super::behave::{Beh, Trig};
use crate::server::{DIMENSIONS, Server};

/// Dimension of a remembered location: the game's three without lock, the others interned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DimKey(u16);

static OTHER_DIMS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

impl DimKey {
    pub fn of(name: &str) -> DimKey {
        if let Some(i) = DIMENSIONS.iter().position(|d| *d == name) {
            return DimKey(i as u16);
        }
        let mut o = OTHER_DIMS.lock().unwrap_or_else(|e| e.into_inner());
        let i = o.iter().position(|d| d == name).unwrap_or_else(|| {
            o.push(name.to_owned());
            o.len() - 1
        });
        DimKey((DIMENSIONS.len() + i) as u16)
    }

    pub fn name(self) -> String {
        let i = usize::from(self.0);
        if i < DIMENSIONS.len() {
            return DIMENSIONS[i].to_owned();
        }
        let o = OTHER_DIMS.lock().unwrap_or_else(|e| e.into_inner());
        o.get(i - DIMENSIONS.len()).cloned().unwrap_or_else(|| DIMENSIONS[0].to_owned())
    }
}

/// Remembered location (home, job site, meeting point): dimension and block, like the game's save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GPos {
    pub dim: DimKey,
    pub pos: [i32; 3],
}

/// Activities (ordering between activities of the same priority: core first; the game
/// follows the order of an identity hash table, non-reproducible).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Act {
    Core,
    Work,
    Play,
    Meet,
    Rest,
    Idle,
    Panic,
    PreRaid,
    Raid,
    Hide,
}

/// Followed target (following a block position / following an entity).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Track {
    Block([i32; 3]),
    /// Entity, followed at eye height or not.
    Ent(Ent, bool),
    /// Ground item (id), followed at the height of its eyes or not.
    Item(i32, bool),
}

/// Walk target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Walk {
    pub target: Track,
    pub speed: f32,
    pub close: i32,
}

/// Memory keys (behavior entry conditions).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum M {
    Walk,
    Look,
    Path,
    Home,
    JobSite,
    PotentialJobSite,
    MeetingPoint,
    Secondary,
    Living,
    Visible,
    Interaction,
    Breed,
    Babies,
    NearestBed,
    WantedItem,
    VisiblePlayer,
}

/// Expected state of a memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum St {
    Present,
    Absent,
}

/// Villager memories.
#[derive(Debug, Clone, Default)]
pub struct Mem {
    pub walk: Option<Walk>,
    pub look: Option<Track>,
    /// Path in progress (the navigation carries the path itself).
    pub path: bool,
    pub cant_reach_since: Option<i64>,
    pub home: Option<GPos>,
    pub job_site: Option<GPos>,
    pub potential_job_site: Option<GPos>,
    pub meeting_point: Option<GPos>,
    pub secondary: Option<Vec<[i32; 3]>>,
    pub living: Option<Vec<Ent>>,
    /// Living entities visible: same list, visibility evaluated lazily (sensor cache).
    pub visible: Option<Vec<(Ent, Option<bool>)>>,
    pub players: Option<Vec<Ent>>,
    pub visible_player: Option<Ent>,
    pub wanted_item: Option<i32>,
    pub nearest_bed: Option<[i32; 3]>,
    pub hurt_by: bool,
    pub hurt_by_entity: Option<Ent>,
    pub hostile: Option<Ent>,
    pub babies: Option<Vec<Ent>>,
    /// Golem seen recently: remaining lifetime.
    pub golem: Option<i64>,
    pub interaction: Option<Ent>,
    pub breed: Option<Ent>,
    pub last_slept: Option<i64>,
    pub last_woken: Option<i64>,
    pub last_worked: Option<i64>,
    pub heard_bell: Option<i64>,
    pub hiding_place: Option<[i32; 3]>,
    pub doors: Option<Vec<[i32; 3]>>,
}

impl Mem {
    pub fn present(&self, k: M) -> bool {
        match k {
            M::Walk => self.walk.is_some(),
            M::Look => self.look.is_some(),
            M::Path => self.path,
            M::Home => self.home.is_some(),
            M::JobSite => self.job_site.is_some(),
            M::PotentialJobSite => self.potential_job_site.is_some(),
            M::MeetingPoint => self.meeting_point.is_some(),
            M::Secondary => self.secondary.is_some(),
            M::Living => self.living.is_some(),
            M::Visible => self.visible.is_some(),
            M::Interaction => self.interaction.is_some(),
            M::Breed => self.breed.is_some(),
            M::Babies => self.babies.is_some(),
            M::NearestBed => self.nearest_bed.is_some(),
            M::WantedItem => self.wanted_item.is_some(),
            M::VisiblePlayer => self.visible_player.is_some(),
        }
    }

    pub fn erase(&mut self, k: M) {
        match k {
            M::Walk => self.walk = None,
            M::Look => self.look = None,
            M::Path => self.path = false,
            M::Home => self.home = None,
            M::JobSite => self.job_site = None,
            M::PotentialJobSite => self.potential_job_site = None,
            M::MeetingPoint => self.meeting_point = None,
            M::Secondary => self.secondary = None,
            M::Living => self.living = None,
            M::Visible => self.visible = None,
            M::Interaction => self.interaction = None,
            M::Breed => self.breed = None,
            M::Babies => self.babies = None,
            M::NearestBed => self.nearest_bed = None,
            M::WantedItem => self.wanted_item = None,
            M::VisiblePlayer => self.visible_player = None,
        }
    }

    pub fn check(&self, e: &[(M, St)]) -> bool {
        e.iter().all(|&(k, st)| self.present(k) == (st == St::Present))
    }

    /// Aging: only the "golem seen" memory expires (599 ticks).
    fn tick(&mut self) {
        if let Some(t) = &mut self.golem
            && *t != i64::MAX
        {
            if *t <= 0 {
                self.golem = None;
            } else {
                *t -= 1;
            }
        }
    }
}

/// Behavior node.
#[derive(Debug, Clone)]
pub enum Node {
    /// Behavior with a duration: state, planned end.
    B { b: Beh, run: bool, end: i64 },
    /// Instant trigger (`OneShot`).
    O { t: Trig, run: bool },
    /// Gate.
    G(Box<Gate>),
    /// Nothing for a duration (`DoNothing`).
    Idle { min: i32, max: i32, run: bool, end: i64 },
}

#[derive(Debug, Clone)]
pub struct Gate {
    pub entry: Vec<(M, St)>,
    pub erase: Vec<M>,
    pub shuffled: bool,
    pub run_one: bool,
    pub kids: Vec<(Node, i32, f64)>,
    pub rng: JRand,
    pub run: bool,
}

impl Gate {
    fn shuffle(&mut self) {
        for k in &mut self.kids {
            let r = self.rng.next_float();
            k.2 = -f64::from(r).powf(f64::from(1.0f32 / k.1 as f32));
        }
        // Stable sort (the game's `List.sort`).
        self.kids.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
    }
}

pub fn run_one(kids: Vec<(Node, i32)>) -> Node {
    gate(Vec::new(), Vec::new(), true, kids)
}

pub fn gate(entry: Vec<(M, St)>, erase: Vec<M>, shuffled: bool, kids: Vec<(Node, i32)>) -> Node {
    Node::G(Box::new(Gate {
        entry,
        erase,
        shuffled,
        run_one: true,
        kids: kids.into_iter().map(|(n, w)| (n, w, 0.0)).collect(),
        rng: JRand::from_time(0x5255_4E4F),
        run: false,
    }))
}

pub fn b(b: Beh) -> Node {
    Node::B { b, run: false, end: 0 }
}

pub fn o(t: Trig) -> Node {
    Node::O { t, run: false }
}

pub fn idle(min: i32, max: i32) -> Node {
    Node::Idle { min, max, run: false, end: 0 }
}

/// Brain state outside behaviors.
#[derive(Debug, Clone)]
pub struct Brain {
    pub mem: Mem,
    /// Active non-core activity, default activity, available activities.
    pub active: Act,
    pub default: Act,
    pub available: Vec<Act>,
    pub baby: bool,
    pub last_schedule: i64,
    /// Sensors: countdown per sensor (game order).
    pub sensors: [i64; 9],
    /// Closest-bed sensor: batch cache, tries, last update.
    pub bed_cache: Vec<([i32; 3], i64)>,
    pub bed_tried: i32,
    pub bed_last: i64,
    /// Rebuild requested (profession change) during the tick.
    pub refresh: bool,
    pub nodes: Vec<(i32, Act, Node)>,
}

/// Sensor periods: living, players, items, bed, hurt, hostiles, babies, secondary POIs (40),
/// golem (200).
const SENSOR_RATES: [i64; 9] = [20, 20, 20, 20, 20, 20, 20, 40, 200];

impl Brain {
    pub fn activity_ok(&self, a: Act) -> bool {
        if a == Act::Core || !self.available.contains(&a) {
            return false;
        }
        match a {
            Act::Work => self.mem.job_site.is_some(),
            Act::Meet => self.mem.meeting_point.is_some(),
            _ => true,
        }
    }

    pub fn is_active(&self, a: Act) -> bool {
        a == Act::Core || self.active == a
    }

    pub fn set_active_if_possible(&mut self, a: Act) {
        if self.activity_ok(a) {
            self.active = a;
        } else {
            self.active = self.default;
        }
    }

    /// Activity according to the schedule (at most every 20 ticks). The timeline follows the Overworld clock
    /// in all dimensions (it is universal).
    pub fn update_schedule(&mut self, game_time: i64, day: i64) {
        if game_time - self.last_schedule > 20 {
            self.last_schedule = game_time;
            let a = schedule(self.baby, day);
            if !self.is_active(a) {
                self.set_active_if_possible(a);
            }
        }
    }
}

/// Timeline `villager_schedule` (period 24000, step values, wrapped).
pub fn schedule(baby: bool, day: i64) -> Act {
    let t = day.rem_euclid(24000);
    let keys: &[(i64, Act)] = if baby {
        &[(10, Act::Idle), (3000, Act::Play), (6000, Act::Idle), (10000, Act::Play), (12000, Act::Rest)]
    } else {
        &[(10, Act::Idle), (2000, Act::Work), (9000, Act::Meet), (11000, Act::Idle), (12000, Act::Rest)]
    };
    let mut v = keys[keys.len() - 1].1;
    for &(k, a) in keys {
        if t >= k {
            v = a;
        }
    }
    v
}

/// Memories kept from one brain to the next (rebuild, save): locations, timestamps,
/// golem seen; the rest is recomputed by the sensors.
pub fn pack(m: &Mem) -> Mem {
    Mem {
        home: m.home,
        job_site: m.job_site,
        potential_job_site: m.potential_job_site,
        meeting_point: m.meeting_point,
        golem: m.golem,
        last_slept: m.last_slept,
        last_woken: m.last_woken,
        last_worked: m.last_worked,
        ..Mem::default()
    }
}

impl Server {
    /// New brain: sensors staggered on the mob's random, profession
    /// activities, schedule applied.
    pub(crate) fn make_brain(&self, m: &mut Mob, mem: Mem) -> Brain {
        let mut sensors = [0i64; 9];
        for (i, r) in SENSOR_RATES.iter().enumerate() {
            sensors[i] = i64::from(m.rng.next_int_bound(*r as i32));
        }
        let baby = m.is_baby();
        let prof = m.villager[1];
        let mut available =
            vec![Act::Core, Act::Meet, Act::Rest, Act::Idle, Act::Panic, Act::PreRaid, Act::Raid, Act::Hide];
        available.insert(0, if baby { Act::Play } else { Act::Work });
        let nodes = super::behave::packages(baby, prof);
        let mut b = Brain {
            mem,
            active: Act::Idle,
            default: Act::Idle,
            available,
            baby,
            last_schedule: -9999,
            sensors,
            bed_cache: Vec::new(),
            bed_tried: 0,
            bed_last: 0,
            refresh: false,
            nodes,
        };
        let t = self.time;
        let day = self.day_time();
        b.update_schedule(t, day);
        b
    }

    /// Dimension of this level, for remembered locations.
    pub(crate) fn dim_key(&self) -> DimKey {
        DimKey::of(&self.dim.name)
    }

    /// Remembered location of this dimension (the others are not reachable from here).
    pub(crate) fn here(&self, g: Option<GPos>) -> Option<[i32; 3]> {
        g.filter(|g| g.dim == self.dim_key()).map(|g| g.pos)
    }

    /// Location of this dimension.
    pub(crate) fn gpos_here(&self, pos: [i32; 3]) -> GPos {
        GPos { dim: self.dim_key(), pos }
    }

    /// Rebuilds the brain: behaviors stopped, memories kept.
    pub(crate) fn refresh_brain(&mut self, m: &mut Mob, mut b: Brain) -> Brain {
        let t = self.time;
        for i in 0..b.nodes.len() {
            let mut n = std::mem::replace(&mut b.nodes[i].2, idle(0, 0));
            self.node_stop_all(m, &mut b, &mut n, t);
            b.nodes[i].2 = n;
        }
        let mem = pack(&b.mem);
        self.make_brain(m, mem)
    }

    /// A brain tick.
    pub(crate) fn brain_tick(&mut self, m: &mut Mob, b: &mut Brain) {
        let t = self.time;
        b.mem.tick();
        self.sensors_tick(m, b);
        // Start of the stopped behaviors of the active activities.
        for i in 0..b.nodes.len() {
            let act = b.nodes[i].1;
            if !b.is_active(act) {
                continue;
            }
            let mut n = std::mem::replace(&mut b.nodes[i].2, idle(0, 0));
            if !is_running(&n) {
                self.node_try_start(m, b, &mut n, t);
            }
            b.nodes[i].2 = n;
            if b.refresh {
                return;
            }
        }
        // Tick or stop of those running.
        for i in 0..b.nodes.len() {
            let mut n = std::mem::replace(&mut b.nodes[i].2, idle(0, 0));
            if is_running(&n) {
                self.node_tick_or_stop(m, b, &mut n, t);
            }
            b.nodes[i].2 = n;
            if b.refresh {
                return;
            }
        }
    }

    fn node_try_start(&mut self, m: &mut Mob, b: &mut Brain, n: &mut Node, t: i64) -> bool {
        match n {
            Node::B { b: beh, run, end } => {
                if !b.mem.check(&beh.entry()) || !self.beh_check(m, b, beh) {
                    return false;
                }
                *run = true;
                let (min, max) = beh.duration();
                let d = i64::from(min) + i64::from(self.mobs.rng.next_int_bound(max + 1 - min));
                *end = t + d;
                self.beh_start(m, b, beh, t);
                true
            }
            Node::O { t: trig, run } => {
                if self.trig(m, b, trig, t) {
                    *run = true;
                    true
                } else {
                    false
                }
            }
            Node::Idle { min, max, run, end } => {
                *run = true;
                let d = i64::from(*min) + i64::from(self.mobs.rng.next_int_bound(*max + 1 - *min));
                *end = t + d;
                true
            }
            Node::G(g) => {
                if !b.mem.check(&g.entry) {
                    return false;
                }
                g.run = true;
                if g.shuffled {
                    g.shuffle();
                }
                for k in 0..g.kids.len() {
                    if is_running(&g.kids[k].0) {
                        continue;
                    }
                    let mut kid = std::mem::replace(&mut g.kids[k].0, idle(0, 0));
                    let ok = self.node_try_start(m, b, &mut kid, t);
                    g.kids[k].0 = kid;
                    if b.refresh || (ok && g.run_one) {
                        break;
                    }
                }
                true
            }
        }
    }

    fn node_tick_or_stop(&mut self, m: &mut Mob, b: &mut Brain, n: &mut Node, t: i64) {
        match n {
            Node::B { b: beh, run, end } => {
                if (t <= *end || !beh.can_time_out()) && self.beh_continue(m, b, beh, t) {
                    self.beh_tick(m, b, beh, t);
                    return;
                }
                *run = false;
                self.beh_stop(m, b, beh, t);
            }
            Node::O { run, .. } => *run = false,
            Node::Idle { run, end, .. } => {
                if t > *end {
                    *run = false;
                }
            }
            Node::G(g) => {
                for k in 0..g.kids.len() {
                    if !is_running(&g.kids[k].0) {
                        continue;
                    }
                    let mut kid = std::mem::replace(&mut g.kids[k].0, idle(0, 0));
                    self.node_tick_or_stop(m, b, &mut kid, t);
                    g.kids[k].0 = kid;
                    if b.refresh {
                        return;
                    }
                }
                if !g.kids.iter().any(|k| is_running(&k.0)) {
                    self.gate_stop(m, b, g, t);
                }
            }
        }
    }

    fn gate_stop(&mut self, m: &mut Mob, b: &mut Brain, g: &mut Gate, t: i64) {
        g.run = false;
        for k in 0..g.kids.len() {
            if is_running(&g.kids[k].0) {
                let mut kid = std::mem::replace(&mut g.kids[k].0, idle(0, 0));
                self.node_stop_all(m, b, &mut kid, t);
                g.kids[k].0 = kid;
            }
        }
        for &e in &g.erase {
            b.mem.erase(e);
        }
    }

    /// The stop of a running node's behavior.
    pub(crate) fn node_stop_all(&mut self, m: &mut Mob, b: &mut Brain, n: &mut Node, t: i64) {
        match n {
            Node::B { b: beh, run, .. } if *run => {
                *run = false;
                self.beh_stop(m, b, beh, t);
            }
            Node::O { run, .. } | Node::Idle { run, .. } => *run = false,
            Node::G(g) if g.run => self.gate_stop(m, b, g, t),
            _ => {}
        }
    }

    // ---------- Sensors ----------

    fn sensors_tick(&mut self, m: &mut Mob, b: &mut Brain) {
        for (i, rate) in SENSOR_RATES.iter().enumerate() {
            b.sensors[i] -= 1;
            if b.sensors[i] > 0 {
                continue;
            }
            b.sensors[i] = *rate;
            // Range of the targeting conditions shared by all brains (the game's static setting
            // rewritten by each sensor that ticks).
            self.mobs.pig.sensor_range = m.follow_range();
            match i {
                0 => self.sense_living(m, b),
                1 => self.sense_players(m, b),
                2 => self.sense_items(m, b),
                3 => self.sense_bed(m, b),
                4 => self.sense_hurt(m, b),
                5 => self.sense_hostiles(m, b),
                6 => self.sense_babies(m, b),
                7 => self.sense_secondary(m, b),
                _ => sense_golem(self, b),
            }
        }
    }

    /// Living entities in the enlarged tracking box, sorted by distance².
    fn sense_living(&mut self, m: &mut Mob, b: &mut Brain) {
        let r = m.follow_range();
        let bb = m.bbox();
        let inside = |p: [f64; 3], w: f64, h: f64| {
            p[0] + w / 2.0 > bb.min[0] - r
                && p[0] - w / 2.0 < bb.max[0] + r
                && p[1] + h > bb.min[1] - r
                && p[1] < bb.max[1] + r
                && p[2] + w / 2.0 > bb.min[2] - r
                && p[2] - w / 2.0 < bb.max[2] + r
        };
        let mut list: Vec<(f64, Ent)> = Vec::new();
        for o in self.mobs.list.iter().flatten() {
            if o.id != m.id && o.alive() && inside(o.pos, f64::from(o.width()), f64::from(o.height())) {
                list.push((dist2(m.pos, o.pos), Ent::Mob(o.id)));
            }
        }
        if self.ent_alive(Ent::Player) && inside(self.pos, 0.6, 1.8) {
            list.push((dist2(m.pos, self.pos), Ent::Player));
        }
        list.sort_by(|a, c| a.0.partial_cmp(&c.0).unwrap_or(std::cmp::Ordering::Equal));
        let ents: Vec<Ent> = list.into_iter().map(|x| x.1).collect();
        b.mem.living = (!ents.is_empty()).then(|| ents.clone());
        // The empty list stays a present memory for the visible living entities (non-empty object).
        b.mem.visible = Some(ents.into_iter().map(|e| (e, None)).collect());
    }

    /// Lazy visibility of a living entity of the list.
    pub(crate) fn visible(&self, m: &mut Mob, b: &mut Brain, e: Ent) -> bool {
        let Some(v) = &mut b.mem.visible else { return false };
        let Some(i) = v.iter().position(|x| x.0 == e) else { return false };
        if let Some(r) = v[i].1 {
            return r;
        }
        let range = self.mobs.pig.sensor_range;
        let r = self.targeting_ok(m, e, false, range, true, true);
        if let Some(v) = &mut b.mem.visible {
            v[i].1 = Some(r);
        }
        r
    }

    /// First visible living entity satisfying the filter.
    pub(crate) fn find_closest(&self, m: &mut Mob, b: &mut Brain, f: impl Fn(&Server, Ent) -> bool) -> Option<Ent> {
        let list: Vec<Ent> = b.mem.visible.as_ref()?.iter().map(|x| x.0).collect();
        list.into_iter().find(|&e| f(self, e) && self.visible(m, b, e))
    }

    fn sense_players(&mut self, m: &mut Mob, b: &mut Brain) {
        let r = m.follow_range();
        let mut players = Vec::new();
        if self.game_mode != 3 && self.ent_alive(Ent::Player) && dist2(m.pos, self.pos) < r * r {
            players.push(Ent::Player);
        }
        b.mem.players = (!players.is_empty()).then(|| players.clone());
        let sr = self.mobs.pig.sensor_range;
        let vis = players.first().copied().filter(|&p| self.targeting_ok(m, p, false, sr, true, true));
        b.mem.visible_player = vis;
    }

    fn sense_items(&mut self, m: &mut Mob, b: &mut Brain) {
        b.mem.wanted_item = self.nearest_wanted_item(m, |st| self.villager_wants(m, st));
    }

    /// Closest bed (babies only): batches of 5, 40-tick cache.
    fn sense_bed(&mut self, m: &mut Mob, b: &mut Brain) {
        if !m.is_baby() {
            return;
        }
        b.bed_tried = 0;
        b.bed_last = self.time + i64::from(self.mobs.rng.next_int_bound(20));
        let center = m.block_pos();
        let all = self.poi_in_range(|t| t == crate::poi::PoiType::Home, center, 48, crate::poi::Occupancy::Any);
        let mut pois = Vec::new();
        for r in all {
            if b.bed_cache.iter().any(|c| c.0 == r.pos) {
                continue;
            }
            b.bed_tried += 1;
            if b.bed_tried >= 5 {
                continue;
            }
            b.bed_cache.push((r.pos, b.bed_last + 40));
            pois.push((r.ty, r.pos));
        }
        let path = self.path_to_pois(m, &pois);
        if let Some(p) = path.filter(|p| p.reached) {
            if self.poi_type(p.target).is_some() {
                b.mem.nearest_bed = Some(p.target);
            }
        } else if b.bed_tried < 5 {
            let last = b.bed_last;
            b.bed_cache.retain(|c| c.1 >= last);
        }
    }

    fn sense_hurt(&mut self, m: &mut Mob, b: &mut Brain) {
        match &m.last_hit {
            Some(h) if self.tick.saturating_sub(h.tick) <= 40 => {
                b.mem.hurt_by = true;
                if let Some(by) = h.by {
                    b.mem.hurt_by_entity = Some(by);
                }
            }
            _ => b.mem.hurt_by = false,
        }
        if let Some(e) = b.mem.hurt_by_entity
            && !self.ent_alive(e)
        {
            b.mem.hurt_by_entity = None;
        }
    }

    fn sense_hostiles(&mut self, m: &mut Mob, b: &mut Brain) {
        let me = m.pos;
        let found = self.find_closest(m, b, |s, e| {
            let Ent::Mob(id) = e else { return false };
            let Some(o) = s.mob_ref(id) else { return false };
            let Some(d) = hostile_distance(o.kind.info().name) else { return false };
            dist2(o.pos, me) <= d * d
        });
        b.mem.hostile = found;
    }

    fn sense_babies(&mut self, m: &mut Mob, b: &mut Brain) {
        let list: Vec<Ent> = b.mem.visible.as_ref().map(|v| v.iter().map(|x| x.0).collect()).unwrap_or_default();
        let mut out = Vec::new();
        for e in list {
            let Ent::Mob(id) = e else { continue };
            let baby_villager =
                self.mob_ref(id).is_some_and(|o| o.kind == super::super::species::Kind::Villager && o.is_baby());
            if baby_villager && self.visible(m, b, e) {
                out.push(e);
            }
        }
        b.mem.babies = (!out.is_empty()).then_some(out);
    }

    /// Secondary POIs of the profession (farmer's farmland): cube ±4, ±2.
    fn sense_secondary(&mut self, m: &mut Mob, b: &mut Brain) {
        let c = m.block_pos();
        let farmer = m.villager[1] == super::PROF_FARMER;
        let mut out = Vec::new();
        if farmer {
            for x in -4..=4 {
                for y in -2..=2 {
                    for z in -4..=4 {
                        let p = [c[0] + x, c[1] + y, c[2] + z];
                        if self.bname(p) == "minecraft:farmland" {
                            out.push(p);
                        }
                    }
                }
            }
        }
        b.mem.secondary = (!out.is_empty()).then_some(out);
    }
}

fn sense_golem(s: &Server, b: &mut Brain) {
    let Some(l) = &b.mem.living else { return };
    let golem = l.iter().any(|e| match e {
        Ent::Mob(id) => s.mob_ref(*id).is_some_and(|o| o.kind.info().name == "minecraft:iron_golem"),
        Ent::Player => false,
    });
    if golem {
        b.mem.golem = Some(599);
    }
}

/// Alert distance of the villager's hostiles.
pub fn hostile_distance(name: &str) -> Option<f64> {
    Some(match name.trim_start_matches("minecraft:") {
        "drowned" | "husk" | "vex" | "zombie" | "zombie_villager" => 8.0,
        "evoker" | "illusioner" | "ravager" => 12.0,
        "pillager" => 15.0,
        "vindicator" | "zoglin" => 10.0,
        _ => return None,
    })
}

pub fn is_running(n: &Node) -> bool {
    match n {
        Node::B { run, .. } | Node::O { run, .. } | Node::Idle { run, .. } => *run,
        Node::G(g) => g.run,
    }
}

pub fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

// ---------- Saving ----------

fn gpos(g: GPos) -> Nbt {
    Nbt::Compound(vec![("pos".into(), Nbt::IntArray(g.pos.to_vec())), ("dimension".into(), Nbt::String(g.dim.name()))])
}

/// Location read back: position and dimension are mandatory (otherwise the memory is lost).
fn read_gpos(v: &Nbt) -> Option<GPos> {
    let v = v.get("value")?;
    let pos = match v.get("pos")? {
        Nbt::IntArray(a) if a.len() == 3 => [a[0], a[1], a[2]],
        _ => return None,
    };
    let Nbt::String(d) = v.get("dimension")? else { return None };
    let dim = rc_data::ids::with_full_id(d, DimKey::of);
    Some(GPos { dim, pos })
}

pub fn mem_to_nbt(m: &Mem) -> Nbt {
    let mut f: Vec<(String, Nbt)> = Vec::new();
    let val = |v: Nbt| Nbt::Compound(vec![("value".into(), v)]);
    if let Some(p) = m.home {
        f.push(("minecraft:home".into(), val(gpos(p))));
    }
    if let Some(p) = m.job_site {
        f.push(("minecraft:job_site".into(), val(gpos(p))));
    }
    if let Some(p) = m.potential_job_site {
        f.push(("minecraft:potential_job_site".into(), val(gpos(p))));
    }
    if let Some(p) = m.meeting_point {
        f.push(("minecraft:meeting_point".into(), val(gpos(p))));
    }
    if let Some(t) = m.golem {
        f.push((
            "minecraft:golem_detected_recently".into(),
            Nbt::Compound(vec![("value".into(), Nbt::Byte(1)), ("ttl".into(), Nbt::Long(t))]),
        ));
    }
    for (k, v) in [("last_slept", m.last_slept), ("last_woken", m.last_woken), ("last_worked_at_poi", m.last_worked)] {
        if let Some(t) = v {
            f.push((format!("minecraft:{k}"), val(Nbt::Long(t))));
        }
    }
    Nbt::Compound(vec![("memories".into(), Nbt::Compound(f))])
}

pub fn mem_from_nbt(v: Option<&Nbt>) -> Mem {
    let mut m = Mem::default();
    let Some(Nbt::Compound(f)) = v.and_then(|v| v.get("memories")) else { return m };
    for (k, v) in f {
        let long = || v.get("value").and_then(Nbt::as_i64);
        match k.as_str() {
            "minecraft:home" => m.home = read_gpos(v),
            "minecraft:job_site" => m.job_site = read_gpos(v),
            "minecraft:potential_job_site" => m.potential_job_site = read_gpos(v),
            "minecraft:meeting_point" => m.meeting_point = read_gpos(v),
            "minecraft:golem_detected_recently" => {
                m.golem = Some(v.get("ttl").and_then(Nbt::as_i64).unwrap_or(i64::MAX))
            }
            "minecraft:last_slept" => m.last_slept = long(),
            "minecraft:last_woken" => m.last_woken = long(),
            "minecraft:last_worked_at_poi" => m.last_worked = long(),
            _ => {}
        }
    }
    m
}
