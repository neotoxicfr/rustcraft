//! Redstone (§ 3): signal reads, dust (default evaluator and order of the hash set),
//! repeaters, comparators, torches, observers, levers, buttons, plates, lamps,
//! doors, trapdoors, fence gates, dispensers, hoppers, TNT, note blocks, bulbs, targets.

use super::table::View;
use super::{Dir, JavaMap, Kind, P, above, below, flag, rel};
use crate::mobs::mob::Ent;
use crate::server::Server;

/// Connection of a dust side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum Side {
    None,
    Side,
    Up,
}

impl Side {
    fn name(self) -> &'static str {
        match self {
            Side::None => "none",
            Side::Side => "side",
            Side::Up => "up",
        }
    }

    fn parse(s: Option<&str>) -> Side {
        match s {
            Some("side") => Side::Side,
            Some("up") => Side::Up,
            _ => Side::None,
        }
    }

    fn connected(self) -> bool {
        self != Side::None
    }
}

/// Direction of the attachment of a block fixed to a face (lever, button): towards which it points.
pub(crate) fn connected_dir(v: &View, s: u32) -> Dir {
    match v.get(s, "face") {
        Some("ceiling") => Dir::Down,
        Some("floor") => Dir::Up,
        _ => v.facing(s),
    }
}

/// `RUSTCRAFT_OPT_WIRESIDE=0`: weak dust signal from the recomputed full state
/// (old path), read once.
fn wireside_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_WIRESIDE") != Some("0"))
}

impl Server {
    // --- Sources and reads (§ 3.1). ---

    pub(crate) fn is_signal_source(&self, s: u32) -> bool {
        let v = self.v();
        if v.kind(s) == Kind::Wire {
            return self.bs.wire_signal;
        }
        v.sim(s) & rc_data::sim_bits::SIGNAL_SOURCE != 0
    }

    /// Weak signal given by state `s` at `p` to the neighbour located at `p - d`.
    pub(crate) fn weak(&self, s: u32, p: P, d: Dir) -> i32 {
        let v = self.v();
        match v.kind(s) {
            Kind::Wire => self.wire_weak(s, p, d),
            Kind::Repeater | Kind::Comparator => {
                if v.facing(s) == d {
                    self.diode_own(s, p)
                } else {
                    0
                }
            }
            Kind::RedstoneTorch => {
                if d != Dir::Up && v.get_bool(s, "lit") {
                    15
                } else {
                    0
                }
            }
            Kind::RedstoneWallTorch => {
                if v.facing(s) != d && v.get_bool(s, "lit") {
                    15
                } else {
                    0
                }
            }
            Kind::Observer => {
                if v.facing(s) == d && v.get_bool(s, "powered") {
                    15
                } else {
                    0
                }
            }
            Kind::Lever | Kind::Button { .. } => {
                if v.get_bool(s, "powered") {
                    15
                } else {
                    0
                }
            }
            Kind::Plate { .. } | Kind::WeightedPlate { .. } => self.plate_signal(s),
            Kind::RedstoneBlock => 15,
            Kind::Target | Kind::DaylightDetector => v.get_int(s, "power"),
            Kind::DetectorRail => {
                if v.get_bool(s, "powered") {
                    15
                } else {
                    0
                }
            }
            Kind::Jukebox => self.jukebox_signal(p), // G6-F01: 15 during playback
            _ => {
                // Other sources (lightning rod, tripwire hook, sensors…): usual properties.
                if !self.is_signal_source(s) {
                    0
                } else if v.has(s, "power") {
                    // Calibrated sensor: nothing towards its input (back).
                    if self.sensor_signal_blocked(s, d) { 0 } else { v.get_int(s, "power") }
                } else if v.get_bool(s, "powered") {
                    15
                } else {
                    0
                }
            }
        }
    }

    /// Strong signal (which charges the receiving block).
    pub(crate) fn strong(&self, s: u32, p: P, d: Dir) -> i32 {
        let v = self.v();
        match v.kind(s) {
            Kind::Wire => {
                if self.bs.wire_signal {
                    self.wire_weak(s, p, d)
                } else {
                    0
                }
            }
            Kind::Repeater | Kind::Comparator | Kind::Observer => self.weak(s, p, d),
            Kind::RedstoneTorch | Kind::RedstoneWallTorch => {
                if d == Dir::Down {
                    self.weak(s, p, d)
                } else {
                    0
                }
            }
            Kind::Lever | Kind::Button { .. } => {
                if v.get_bool(s, "powered") && connected_dir(&v, s) == d {
                    15
                } else {
                    0
                }
            }
            Kind::Plate { .. } | Kind::WeightedPlate { .. } => {
                if d == Dir::Up {
                    self.plate_signal(s)
                } else {
                    0
                }
            }
            Kind::DetectorRail => {
                if d == Dir::Up && v.get_bool(s, "powered") {
                    15
                } else {
                    0
                }
            }
            Kind::RedstoneBlock | Kind::Target | Kind::DaylightDetector | Kind::Jukebox => 0,
            // G6-F01: lectern page pulse, strong towards the block below.
            Kind::Lectern => {
                if d == Dir::Up && v.get_bool(s, "powered") {
                    15
                } else {
                    0
                }
            }
            _ => {
                let n = v.d.block_of(s).map_or("", |b| b.name.as_str());
                // The eight lightning rods (oxidation stages, waxed) emit the same.
                if (n.ends_with("lightning_rod") || n == "minecraft:tripwire_hook") && v.get_bool(s, "powered") {
                    if v.facing(s) == d { 15 } else { 0 }
                } else if n == "minecraft:trapped_chest" || n.ends_with("sculk_sensor") {
                    if d == Dir::Up { self.weak(s, p, d) } else { 0 }
                } else {
                    0
                }
            }
        }
    }

    /// The signal received, as the game computes it: weak, or strong received if the block is a conductor.
    pub(crate) fn signal(&self, q: P, d: Dir) -> i32 {
        let s = self.get(q);
        let w = self.weak(s, q, d);
        if self.v().conductor(s) { w.max(self.direct_signal_to(q)) } else { w }
    }

    pub(crate) fn has_signal(&self, q: P, d: Dir) -> bool {
        self.signal(q, d) > 0
    }

    /// Strongest strong signal received by `p` (down, up, north, south, west, east; stops at 15).
    pub(crate) fn direct_signal_to(&self, p: P) -> i32 {
        let mut r = 0;
        for d in Dir::ALL {
            let q = rel(p, d);
            r = r.max(self.strong(self.get(q), q, d));
            if r >= 15 {
                return r;
            }
        }
        r
    }

    /// A neighbour gives a signal (order down, up, north, south, west, east).
    pub(crate) fn has_neighbor_signal(&self, p: P) -> bool {
        Dir::ALL.iter().any(|&d| self.signal(rel(p, d), d) > 0)
    }

    pub(crate) fn best_neighbor_signal(&self, p: P) -> i32 {
        let mut best = 0;
        for d in Dir::ALL {
            let s = self.signal(rel(p, d), d);
            if s >= 15 {
                return 15;
            }
            best = best.max(s);
        }
        best
    }

    /// Side input.
    pub(crate) fn control_input(&self, q: P, d: Dir, diodes_only: bool) -> i32 {
        let s = self.get(q);
        let v = self.v();
        let k = v.kind(s);
        if diodes_only {
            if matches!(k, Kind::Repeater | Kind::Comparator) { self.strong(s, q, d) } else { 0 }
        } else if k == Kind::RedstoneBlock {
            15
        } else if k == Kind::Wire {
            v.get_int(s, "power")
        } else if self.is_signal_source(s) {
            self.strong(s, q, d)
        } else {
            0
        }
    }

    // --- Dust (§ 3.3). ---

    fn should_wire_connect(&self, s: u32, d: Option<Dir>) -> bool {
        let v = self.v();
        match v.kind(s) {
            Kind::Wire => true,
            Kind::Repeater => d.is_some_and(|d| v.facing(s) == d || v.facing(s).opposite() == d),
            Kind::Observer => d == Some(v.facing(s)),
            _ => self.is_signal_source(s) && d.is_some(),
        }
    }

    fn wire_can_survive_on(&self, s: u32) -> bool {
        self.v().sturdy(s, Dir::Up) || self.v().block(s) == self.bs.t.id.hopper
    }

    fn wire_side(&self, p: P, d: Dir, can_up: bool) -> Side {
        let rp = rel(p, d);
        let rs = self.get(rp);
        if can_up {
            let placeable = matches!(self.v().kind(rs), Kind::Trapdoor { .. }) || self.wire_can_survive_on(rs);
            if placeable && self.should_wire_connect(self.get(above(rp)), None) {
                return if self.v().sturdy(rs, d.opposite()) { Side::Up } else { Side::Side };
            }
        }
        if !self.should_wire_connect(rs, Some(d))
            && (self.v().conductor(rs) || !self.should_wire_connect(self.get(below(rp)), None))
        {
            Side::None
        } else {
            Side::Side
        }
    }

    fn wire_get(&self, s: u32, d: Dir) -> Side {
        Side::parse(self.v().get(s, d.name()))
    }

    fn wire_set(&self, s: u32, d: Dir, side: Side) -> u32 {
        self.v().set(s, d.name(), side.name())
    }

    fn wire_is_dot(&self, s: u32) -> bool {
        Dir::HORIZONTAL.iter().all(|&d| !self.wire_get(s, d).connected())
    }

    fn wire_is_cross(&self, s: u32) -> bool {
        Dir::HORIZONTAL.iter().all(|&d| self.wire_get(s, d).connected())
    }

    fn wire_cross_state(&self) -> u32 {
        let mut s = self.v().default_state(self.bs.t.id.wire);
        for d in Dir::HORIZONTAL {
            s = self.wire_set(s, d, Side::Side);
        }
        s
    }

    fn wire_missing_connections(&self, mut s: u32, p: P) -> u32 {
        let can_up = !self.v().conductor(self.get(above(p)));
        for d in Dir::HORIZONTAL {
            if !self.wire_get(s, d).connected() {
                let side = self.wire_side(p, d, can_up);
                s = self.wire_set(s, d, side);
            }
        }
        s
    }

    /// Connections recomputed, dot or extended line.
    pub(crate) fn wire_connection_state(&self, state: u32, p: P) -> u32 {
        let was_dot = self.wire_is_dot(state);
        let power = self.v().get_int(state, "power");
        let base = self.v().set_int(self.v().default_state(self.bs.t.id.wire), "power", power);
        let mut s = self.wire_missing_connections(base, p);
        if was_dot && self.wire_is_dot(s) {
            return s;
        }
        let n = self.wire_get(s, Dir::North).connected();
        let so = self.wire_get(s, Dir::South).connected();
        let e = self.wire_get(s, Dir::East).connected();
        let w = self.wire_get(s, Dir::West).connected();
        let ns_empty = !n && !so;
        let ew_empty = !e && !w;
        if !w && ns_empty {
            s = self.wire_set(s, Dir::West, Side::Side);
        }
        if !e && ns_empty {
            s = self.wire_set(s, Dir::East, Side::Side);
        }
        if !n && ew_empty {
            s = self.wire_set(s, Dir::North, Side::Side);
        }
        if !so && ew_empty {
            s = self.wire_set(s, Dir::South, Side::Side);
        }
        s
    }

    pub(crate) fn wire_placement(&self, p: P) -> u32 {
        self.wire_connection_state(self.wire_cross_state(), p)
    }

    pub(crate) fn wire_update_shape(&self, s: u32, p: P, dir: Dir, ns: u32) -> u32 {
        if dir == Dir::Down {
            return if self.wire_can_survive_on(ns) { s } else { self.air() };
        }
        if dir == Dir::Up {
            return self.wire_connection_state(s, p);
        }
        let side = self.wire_side(p, dir, !self.v().conductor(self.get(above(p))));
        if side.connected() == self.wire_get(s, dir).connected() && !self.wire_is_cross(s) {
            self.wire_set(s, dir, side)
        } else {
            let power = self.v().get_int(s, "power");
            let cross = self.v().set_int(self.wire_cross_state(), "power", power);
            self.wire_connection_state(self.wire_set(cross, dir, side), p)
        }
    }

    pub(crate) fn wire_indirect_shapes(&mut self, s: u32, p: P, flags: u32, limit: i32) {
        let wire = self.bs.t.id.wire;
        for d in Dir::HORIZONTAL {
            let side = self.wire_get(s, d);
            let np = rel(p, d);
            if side != Side::None && self.v().block(self.get(np)) != wire {
                let down = below(np);
                if self.v().block(self.get(down)) == wire {
                    let q = rel(down, d.opposite());
                    let qs = self.get(q);
                    self.neighbor_shape_changed(d.opposite(), down, q, qs, flags, limit);
                }
                let up = above(np);
                if self.v().block(self.get(up)) == wire {
                    let q = rel(up, d.opposite());
                    let qs = self.get(q);
                    self.neighbor_shape_changed(d.opposite(), up, q, qs, flags, limit);
                }
            }
        }
    }

    fn wire_weak(&self, s: u32, p: P, d: Dir) -> i32 {
        if !self.bs.wire_signal || d == Dir::Down {
            return 0;
        }
        let power = self.v().get_int(s, "power");
        if power == 0 {
            return 0;
        }
        if d != Dir::Up && !self.wire_connected_after_update(s, p, d.opposite()) {
            0
        } else {
            power
        }
    }

    /// Would side `t` of dust `s` at `p` be connected after recomputing the connections?
    /// Same answer as the recomputed full state read on that side (`RUSTCRAFT_OPT_WIRESIDE=0`:
    /// that full computation). Otherwise, without writing a property: the requested side, then the two
    /// perpendicular ones (if one is connected, the line is not extended towards `t`), then
    /// the opposite only when the stored state is a dot (a dot none of whose sides
    /// connects stays a dot). World reads without effect: reading fewer changes nothing.
    fn wire_connected_after_update(&self, s: u32, p: P, t: Dir) -> bool {
        if !wireside_on() {
            return self.wire_get(self.wire_connection_state(s, p), t).connected();
        }
        self.wire_connected_side_only(s, p, t)
    }

    fn wire_connected_side_only(&self, s: u32, p: P, t: Dir) -> bool {
        let can_up = !self.v().conductor(self.get(above(p)));
        if self.wire_side(p, t, can_up).connected() {
            return true;
        }
        if self.wire_side(p, t.clockwise(), can_up).connected()
            || self.wire_side(p, t.counter_clockwise(), can_up).connected()
        {
            return false;
        }
        !self.wire_is_dot(s) || self.wire_side(p, t.opposite(), can_up).connected()
    }

    fn wire_block_signal(&mut self, p: P) -> i32 {
        self.bs.wire_signal = false;
        let s = self.best_neighbor_signal(p);
        self.bs.wire_signal = true;
        s
    }

    fn wire_incoming(&self, p: P) -> i32 {
        let wire = self.bs.t.id.wire;
        let v = self.v();
        let power = |s: u32| if v.block(s) == wire { v.get_int(s, "power") } else { 0 };
        let mut w = 0;
        for d in Dir::HORIZONTAL {
            let np = rel(p, d);
            let ns = self.get(np);
            w = w.max(power(ns));
            if v.conductor(ns) && !v.conductor(self.get(above(p))) {
                w = w.max(power(self.get(above(np))));
            } else if !v.conductor(ns) {
                w = w.max(power(self.get(below(np))));
            }
        }
        (w - 1).max(0)
    }

    fn wire_update_power(&mut self, p: P, state: u32) {
        let bs = self.wire_block_signal(p);
        let target = if bs == 15 { 15 } else { bs.max(self.wire_incoming(p)) };
        if self.v().get_int(state, "power") == target {
            return;
        }
        if self.get(p) == state {
            let ns = self.v().set_int(state, "power", target);
            self.set_block(p, ns, flag::CLIENTS);
        }
        let mut set: JavaMap<()> = JavaMap::new();
        set.insert(p, ());
        for d in Dir::ALL {
            set.insert(rel(p, d), ());
        }
        let wire = self.bs.t.id.wire;
        let order: Vec<P> = set.iter().map(|e| e.0).collect();
        for q in order {
            self.update_neighbors_at(q, wire);
        }
    }

    fn wire_check_corner(&mut self, p: P) {
        let wire = self.bs.t.id.wire;
        if self.v().block(self.get(p)) == wire {
            self.update_neighbors_at(p, wire);
            for d in Dir::ALL {
                self.update_neighbors_at(rel(p, d), wire);
            }
        }
    }

    fn wire_neighbours_of_wires(&mut self, p: P) {
        for d in Dir::HORIZONTAL {
            self.wire_check_corner(rel(p, d));
        }
        for d in Dir::HORIZONTAL {
            let t = rel(p, d);
            if self.v().conductor(self.get(t)) {
                self.wire_check_corner(above(t));
            } else {
                self.wire_check_corner(below(t));
            }
        }
    }

    pub(crate) fn wire_on_place(&mut self, state: u32, p: P) {
        self.wire_update_power(p, state);
        let wire = self.bs.t.id.wire;
        for d in [Dir::Up, Dir::Down] {
            self.update_neighbors_at(rel(p, d), wire);
        }
        self.wire_neighbours_of_wires(p);
    }

    pub(crate) fn wire_on_remove(&mut self, state: u32, p: P) {
        let wire = self.bs.t.id.wire;
        for d in Dir::ALL {
            self.update_neighbors_at(rel(p, d), wire);
        }
        self.wire_update_power(p, state);
        self.wire_neighbours_of_wires(p);
    }

    pub(crate) fn wire_neighbor_changed(&mut self, state: u32, p: P) {
        if self.wire_can_survive_on(self.get(below(p))) {
            self.wire_update_power(p, state);
        } else {
            // Loot dropped before removal.
            self.drop_resources(p, state, None);
            self.remove_block(p, false);
        }
    }

    /// Right click: dot ↔ cross.
    pub(crate) fn wire_use(&mut self, state: u32, p: P) -> bool {
        if !self.wire_is_cross(state) && !self.wire_is_dot(state) {
            return false;
        }
        let power = self.v().get_int(state, "power");
        let base =
            if self.wire_is_cross(state) { self.v().default_state(self.bs.t.id.wire) } else { self.wire_cross_state() };
        let ns = self.wire_connection_state(self.v().set_int(base, "power", power), p);
        if ns == state {
            return false;
        }
        self.set_update(p, ns);
        let wire = self.bs.t.id.wire;
        for d in Dir::HORIZONTAL {
            let rp = rel(p, d);
            if self.wire_get(state, d).connected() != self.wire_get(ns, d).connected()
                && self.v().conductor(self.get(rp))
            {
                self.update_neighbors_except(rp, wire, d.opposite());
            }
        }
        true
    }

    // --- Repeaters and comparators (§ 3.4, § 3.5). ---

    fn diode_own(&self, s: u32, p: P) -> i32 {
        let v = self.v();
        if !v.get_bool(s, "powered") {
            return 0;
        }
        if v.kind(s) == Kind::Comparator { self.be_int(p, "OutputSignal") } else { 15 }
    }

    fn diode_input(&self, p: P, s: u32) -> i32 {
        let d = self.v().facing(s);
        let tp = rel(p, d);
        let input = self.signal(tp, d);
        if input >= 15 {
            return input;
        }
        let ts = self.get(tp);
        let wire = if self.v().kind(ts) == Kind::Wire { self.v().get_int(ts, "power") } else { 0 };
        input.max(wire)
    }

    pub(crate) fn comparator_input(&self, p: P, s: u32) -> i32 {
        let mut r = self.diode_input(p, s);
        let d = self.v().facing(s);
        let mut tp = rel(p, d);
        let ts = self.get(tp);
        if self.v().has_analog(ts) {
            r = self.analog_output(ts, tp, d.opposite());
        } else if r < 15 && self.v().conductor(ts) {
            tp = rel(tp, d);
            let ts = self.get(tp);
            // The strongest of the frame turned towards the comparator and of the analog block.
            let frame = self.frame_signal_at(tp, d);
            let block = self.v().has_analog(ts).then(|| self.analog_output(ts, tp, d.opposite()));
            if let Some(v) = frame.max(block) {
                r = v;
            }
        }
        r
    }

    fn diode_alternate(&self, p: P, s: u32, diodes_only: bool) -> i32 {
        let d = self.v().facing(s);
        let (cw, ccw) = (d.clockwise(), d.counter_clockwise());
        self.control_input(rel(p, cw), cw, diodes_only).max(self.control_input(rel(p, ccw), ccw, diodes_only))
    }

    pub(crate) fn repeater_locked(&self, p: P, s: u32) -> bool {
        self.diode_alternate(p, s, true) > 0
    }

    fn comparator_output(&self, p: P, s: u32) -> i32 {
        let input = self.comparator_input(p, s);
        if input == 0 {
            return 0;
        }
        let alt = self.diode_alternate(p, s, false);
        if alt > input {
            0
        } else if self.v().get(s, "mode") == Some("subtract") {
            input - alt
        } else {
            input
        }
    }

    fn diode_should_turn_on(&self, p: P, s: u32) -> bool {
        if self.v().kind(s) == Kind::Comparator {
            let input = self.comparator_input(p, s);
            if input == 0 {
                return false;
            }
            let side = self.diode_alternate(p, s, false);
            input > side || (input == side && self.v().get(s, "mode") == Some("compare"))
        } else {
            self.diode_input(p, s) > 0
        }
    }

    fn diode_prioritize(&self, p: P, s: u32) -> bool {
        let d = self.v().facing(s).opposite();
        let os = self.get(rel(p, d));
        matches!(self.v().kind(os), Kind::Repeater | Kind::Comparator) && self.v().facing(os) != d
    }

    fn diode_delay(&self, s: u32) -> i32 {
        if self.v().kind(s) == Kind::Comparator { 2 } else { self.v().get_int(s, "delay") * 2 }
    }

    pub(crate) fn diode_update_in_front(&mut self, p: P, s: u32) {
        let d = self.v().facing(s);
        let opp = rel(p, d.opposite());
        let b = self.v().block(s);
        self.neighbor_changed_at(opp, b);
        self.update_neighbors_except(opp, b, d);
    }

    pub(crate) fn diode_neighbor_changed(&mut self, s: u32, p: P) {
        let b = self.v().block(s);
        if self.v().block(self.get(p)) != b {
            return;
        }
        if self.v().rigid_up(self.get(below(p))) {
            self.diode_check_tick(p, s);
        } else {
            // Loot dropped before removal (comparator block entity included).
            self.drop_resources(p, s, None);
            self.remove_block(p, false);
            for d in Dir::ALL {
                self.update_neighbors_at(rel(p, d), b);
            }
        }
    }

    fn diode_check_tick(&mut self, p: P, s: u32) {
        let b = self.v().block(s);
        if self.v().kind(s) == Kind::Comparator {
            self.comparator_unseal(p, s); // S8: sealed loot drawn
            if !self.will_tick_this_tick(p, b) {
                let out = self.comparator_output(p, s);
                let old = self.be_int(p, "OutputSignal");
                if out != old || self.v().get_bool(s, "powered") != self.diode_should_turn_on(p, s) {
                    let prio = if self.diode_prioritize(p, s) { -1 } else { 0 };
                    self.schedule_tick_priority(p, b, 2, prio);
                }
            }
            return;
        }
        if self.repeater_locked(p, s) {
            return;
        }
        let on = self.v().get_bool(s, "powered");
        let should = self.diode_should_turn_on(p, s);
        if on != should && !self.will_tick_this_tick(p, b) {
            let prio = if self.diode_prioritize(p, s) {
                -3
            } else if on {
                -2
            } else {
                -1
            };
            let delay = self.diode_delay(s);
            self.schedule_tick_priority(p, b, delay, prio);
        }
    }

    pub(crate) fn diode_tick(&mut self, s: u32, p: P) {
        if self.repeater_locked(p, s) {
            return;
        }
        let on = self.v().get_bool(s, "powered");
        let should = self.diode_should_turn_on(p, s);
        if on && !should {
            let ns = self.v().set_bool(s, "powered", false);
            self.set_block(p, ns, flag::CLIENTS);
        } else if !on {
            let ns = self.v().set_bool(s, "powered", true);
            self.set_block(p, ns, flag::CLIENTS);
            if !should {
                let b = self.v().block(s);
                let delay = self.diode_delay(s);
                self.schedule_tick_priority(p, b, delay, -2);
            }
        }
    }

    pub(crate) fn comparator_refresh(&mut self, p: P, s: u32) {
        self.comparator_unseal(p, s); // S8: sealed loot drawn
        let out = self.comparator_output(p, s);
        let old = self.be_int(p, "OutputSignal");
        self.be_set_int(p, "OutputSignal", out);
        if old != out || self.v().get(s, "mode") == Some("compare") {
            let source_on = self.diode_should_turn_on(p, s);
            let is_on = self.v().get_bool(s, "powered");
            if is_on && !source_on {
                let ns = self.v().set_bool(s, "powered", false);
                self.set_block(p, ns, flag::CLIENTS);
            } else if !is_on && source_on {
                let ns = self.v().set_bool(s, "powered", true);
                self.set_block(p, ns, flag::CLIENTS);
            }
            self.diode_update_in_front(p, s);
        }
    }

    /// Placement by a player with an active input: tick at 1 (§ 3.4).
    pub(crate) fn diode_placed_by_player(&mut self, p: P, s: u32) {
        if self.v().kind(s) == Kind::Comparator {
            self.comparator_unseal(p, s); // S8: sealed loot drawn
        }
        if self.diode_should_turn_on(p, s) {
            let b = self.v().block(s);
            self.schedule_tick(p, b, 1);
        }
    }

    pub(crate) fn repeater_use(&mut self, p: P, s: u32) {
        let ns = self.v().cycle(s, "delay");
        self.set_update(p, ns);
    }

    pub(crate) fn comparator_use(&mut self, p: P, s: u32) {
        let ns = self.v().cycle(s, "mode");
        self.set_block(p, ns, flag::CLIENTS);
        if self.v().kind(self.get(p)) == Kind::Comparator {
            self.comparator_refresh(p, ns);
        }
    }

    // --- Torches (§ 3.6). ---

    fn torch_input(&self, p: P, s: u32) -> bool {
        if self.v().kind(s) == Kind::RedstoneWallTorch {
            let o = self.v().facing(s).opposite();
            self.has_signal(rel(p, o), o)
        } else {
            self.has_signal(below(p), Dir::Down)
        }
    }

    pub(crate) fn torch_notify(&mut self, p: P, s: u32) {
        let b = self.v().block(s);
        for d in Dir::ALL {
            self.update_neighbors_at(rel(p, d), b);
        }
    }

    pub(crate) fn torch_neighbor_changed(&mut self, s: u32, p: P) {
        let b = self.v().block(s);
        if self.v().get_bool(s, "lit") == self.torch_input(p, s) && !self.will_tick_this_tick(p, b) {
            self.schedule_tick(p, b, 2);
        }
    }

    fn torch_toggled_too_often(&mut self, p: P, add: bool) -> bool {
        if add {
            self.bs.torches.push_back((p, self.time));
        }
        self.bs.torches.iter().filter(|t| t.0 == p).take(8).count() >= 8
    }

    pub(crate) fn torch_tick(&mut self, s: u32, p: P) {
        let input = self.torch_input(p, s);
        while self.bs.torches.front().is_some_and(|t| self.time - t.1 > 60) {
            self.bs.torches.pop_front();
        }
        if self.v().get_bool(s, "lit") {
            if input {
                let ns = self.v().set_bool(s, "lit", false);
                self.set_update(p, ns);
                if self.torch_toggled_too_often(p, true) {
                    self.level_event(1502, p, 0);
                    let b = self.v().block(self.get(p));
                    self.schedule_tick(p, b, 160);
                }
            }
        } else if !input && !self.torch_toggled_too_often(p, false) {
            let ns = self.v().set_bool(s, "lit", true);
            self.set_update(p, ns);
        }
    }

    // --- Observer (§ 3.7). ---

    pub(crate) fn observer_update_shape(&mut self, s: u32, p: P, dir: Dir) -> u32 {
        if self.v().facing(s) == dir && !self.v().get_bool(s, "powered") {
            let b = self.bs.t.id.observer;
            if !self.has_scheduled_tick(p, b) {
                self.schedule_tick(p, b, 2);
            }
        }
        s
    }

    fn observer_in_front(&mut self, p: P, s: u32) {
        let d = self.v().facing(s);
        let opp = rel(p, d.opposite());
        let b = self.bs.t.id.observer;
        self.neighbor_changed_at(opp, b);
        self.update_neighbors_except(opp, b, d);
    }

    pub(crate) fn observer_tick(&mut self, s: u32, p: P) {
        if self.v().get_bool(s, "powered") {
            let ns = self.v().set_bool(s, "powered", false);
            self.set_block(p, ns, flag::CLIENTS);
        } else {
            let ns = self.v().set_bool(s, "powered", true);
            self.set_block(p, ns, flag::CLIENTS);
            let b = self.bs.t.id.observer;
            self.schedule_tick(p, b, 2);
        }
        self.observer_in_front(p, s);
    }

    pub(crate) fn observer_on_place(&mut self, s: u32, p: P) {
        let b = self.bs.t.id.observer;
        if self.v().get_bool(s, "powered") && !self.has_scheduled_tick(p, b) {
            let ns = self.v().set_bool(s, "powered", false);
            self.set_block(p, ns, flag::CLIENTS | flag::KNOWN_SHAPE);
            self.observer_in_front(p, ns);
        }
    }

    pub(crate) fn observer_on_remove(&mut self, s: u32, p: P) {
        let b = self.bs.t.id.observer;
        if self.v().get_bool(s, "powered") && self.has_scheduled_tick(p, b) {
            let ns = self.v().set_bool(s, "powered", false);
            self.observer_in_front(p, ns);
        }
    }

    // --- Levers, buttons, plates (§ 3.8). ---

    pub(crate) fn attached_update_neighbours(&mut self, p: P, s: u32) {
        let front = connected_dir(&self.v(), s).opposite();
        let b = self.v().block(s);
        self.update_neighbors_at(p, b);
        self.update_neighbors_at(rel(p, front), b);
    }

    /// Lever toggled (hand or wind burst): sound for all, event without source.
    pub(crate) fn lever_pull(&mut self, p: P, s: u32) {
        let ns = self.v().cycle(s, "powered");
        self.set_update(p, ns);
        self.attached_update_neighbours(p, ns);
        let on = self.v().get_bool(ns, "powered");
        self.block_sound(p, "minecraft:block.lever.click", 0.3, if on { 0.6 } else { 0.5 });
        let event = if on { "minecraft:block_activate" } else { "minecraft:block_deactivate" };
        self.game_event_block(event, p, None, None);
    }

    /// Button pressed by `by` (player by hand: sound predicted by their client, hence not sent;
    /// wind burst: sound for all), `block_activate` event attributed to `by`.
    pub(crate) fn button_press(&mut self, p: P, s: u32, by: Option<crate::mobs::mob::Ent>) -> bool {
        if self.v().get_bool(s, "powered") {
            return false;
        }
        let ns = self.v().set_bool(s, "powered", true);
        self.set_update(p, ns);
        self.attached_update_neighbours(p, s);
        if let Kind::Button { ticks, .. } = self.v().kind(s) {
            let b = self.v().block(s);
            self.schedule_tick(p, b, i32::from(ticks));
        }
        if by != Some(crate::mobs::mob::Ent::Player) {
            let name = button_sound(self.state_name(s), true);
            self.block_sound(p, &name, 1.0, 1.0);
        }
        self.game_event_block("minecraft:block_activate", p, by, None);
        true
    }

    pub(crate) fn button_tick(&mut self, s: u32, p: P) {
        if self.v().get_bool(s, "powered") {
            self.button_check_pressed(s, p, None);
        }
    }

    /// Arrow that entered a released button (`cur`: the arrow being ticked, absent from the
    /// up-to-date list): only wooden buttons react.
    pub(crate) fn button_arrow_inside(&mut self, p: P, cur: &crate::projectiles::Proj) {
        let s = self.get(p);
        if matches!(self.v().kind(s), Kind::Button { arrows: true, .. }) && !self.v().get_bool(s, "powered") {
            self.button_check_pressed(s, p, Some(cur));
        }
    }

    /// Button pressed as long as an arrow (arrow, spectral arrow, trident) intersects its shape:
    /// state change (neighbours, sound for all, event attributed to the arrow), then tick
    /// rescheduled as long as it stays.
    fn button_check_pressed(&mut self, s: u32, p: P, cur: Option<&crate::projectiles::Proj>) {
        let Kind::Button { ticks, arrows } = self.v().kind(s) else { return };
        let arrow = if arrows { self.button_first_arrow(p, s, cur) } else { None };
        let pressed = arrow.is_some();
        if pressed != self.v().get_bool(s, "powered") {
            let ns = self.v().set_bool(s, "powered", pressed);
            self.set_update(p, ns);
            self.attached_update_neighbours(p, s);
            let name = button_sound(self.state_name(s), pressed);
            self.block_sound(p, &name, 1.0, 1.0);
            let ev = if pressed { "minecraft:block_activate" } else { "minecraft:block_deactivate" };
            let c = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
            let src = arrow.map(crate::mobs::recent::vibration::Src::Projectile);
            self.game_event_src(ev, c, src, None);
        }
        if pressed {
            let b = self.v().block(s);
            self.schedule_tick(p, b, i32::from(ticks));
        }
    }

    /// First arrow whose box intersects the outline of the button: `Some(shooter)`.
    fn button_first_arrow(&self, p: P, s: u32, cur: Option<&crate::projectiles::Proj>) -> Option<Option<Ent>> {
        let boxes = self.data.shape(self.v().info(s).outline);
        if boxes.is_empty() {
            return None;
        }
        let mut lo = [f64::MAX; 3];
        let mut hi = [f64::MIN; 3];
        for b in boxes {
            for i in 0..3 {
                lo[i] = lo[i].min(b.min[i] + f64::from(p[i]));
                hi[i] = hi[i].max(b.max[i] + f64::from(p[i]));
            }
        }
        let hits = |q: &crate::projectiles::Proj| {
            let b = q.bbox();
            !q.removed && q.kind.arrow_like() && (0..3).all(|i| b.max[i] > lo[i] && b.min[i] < hi[i])
        };
        let mut seen_cur = false;
        for q in &self.proj.list {
            let q = match cur {
                Some(c) if c.id == q.id => {
                    seen_cur = true;
                    c
                }
                _ => q,
            };
            if hits(q) {
                return Some(self.proj_owner(q));
            }
        }
        match cur {
            Some(c) if !seen_cur && hits(c) => Some(self.proj_owner(c)),
            _ => None,
        }
    }

    pub(crate) fn plate_signal(&self, s: u32) -> i32 {
        let v = self.v();
        match v.kind(s) {
            Kind::WeightedPlate { .. } => v.get_int(s, "power"),
            _ => {
                if v.get_bool(s, "powered") {
                    15
                } else {
                    0
                }
            }
        }
    }

    pub(crate) fn plate_update_neighbours(&mut self, p: P, s: u32) {
        let b = self.v().block(s);
        self.update_neighbors_at(p, b);
        self.update_neighbors_at(below(p), b);
    }

    fn plate_strength(&self, p: P, s: u32) -> i32 {
        let bx = rc_physics::Aabb {
            min: [f64::from(p[0]) + 0.0625, f64::from(p[1]), f64::from(p[2]) + 0.0625],
            max: [f64::from(p[0]) + 0.9375, f64::from(p[1]) + 0.25, f64::from(p[2]) + 0.9375],
        };
        match self.v().kind(s) {
            Kind::WeightedPlate { max } => {
                let n = self.count_entities_in(&bx, false).min(usize::from(max));
                if n > 0 { (n as f32 / f32::from(max) * 15.0).ceil() as i32 } else { 0 }
            }
            Kind::Plate { mobs_only } if self.count_entities_in(&bx, mobs_only) > 0 => 15,
            _ => 0,
        }
    }

    pub(crate) fn plate_check(&mut self, p: P, s: u32, old: i32) {
        let signal = self.plate_strength(p, s);
        if old != signal {
            let ns = match self.v().kind(s) {
                Kind::WeightedPlate { .. } => self.v().set_int(s, "power", signal),
                _ => self.v().set_bool(s, "powered", signal > 0),
            };
            self.set_block(p, ns, flag::CLIENTS);
            self.plate_update_neighbours(p, s);
        }
        // Press or release click (sound of the block type, for all) and event.
        let (was, is) = (old > 0, signal > 0);
        if was != is {
            let name = plate_sound(self.state_name(s), is);
            self.block_sound(p, &name, 1.0, 1.0);
            let ev = if is { "minecraft:block_activate" } else { "minecraft:block_deactivate" };
            self.game_event_block(ev, p, None, None);
        }
        if signal > 0 {
            let delay = if matches!(self.v().kind(s), Kind::WeightedPlate { .. }) { 10 } else { 20 };
            let b = self.v().block(s);
            self.schedule_tick(p, b, delay);
        }
    }

    pub(crate) fn plate_tick(&mut self, s: u32, p: P) {
        let sig = self.plate_signal(s);
        if sig > 0 {
            self.plate_check(p, s, sig);
        }
    }

    /// Entity in a plate (player or simulated entity): test whether it is at 0.
    pub(crate) fn plate_entity_inside(&mut self, p: P) {
        let s = self.get(p);
        if matches!(self.v().kind(s), Kind::Plate { .. } | Kind::WeightedPlate { .. }) && self.plate_signal(s) == 0 {
            self.plate_check(p, s, 0);
        }
    }

    // --- Lamp (§ 3.12). ---

    pub(crate) fn lamp_neighbor_changed(&mut self, s: u32, p: P) {
        let lit = self.v().get_bool(s, "lit");
        if lit != self.has_neighbor_signal(p) {
            if lit {
                let b = self.v().block(s);
                self.schedule_tick(p, b, 4);
            } else {
                let ns = self.v().cycle(s, "lit");
                self.set_block(p, ns, flag::CLIENTS);
            }
        }
    }

    pub(crate) fn lamp_tick(&mut self, s: u32, p: P) {
        if self.v().get_bool(s, "lit") && !self.has_neighbor_signal(p) {
            let ns = self.v().cycle(s, "lit");
            self.set_block(p, ns, flag::CLIENTS);
        }
    }

    // --- Doors, trapdoors, fence gates (§ 3.11). ---

    pub(crate) fn door_neighbor_changed(&mut self, s: u32, p: P, from: u16) {
        let other = if self.v().get(s, "half") == Some("lower") { above(p) } else { below(p) };
        let signal = self.has_neighbor_signal(p) || self.has_neighbor_signal(other);
        if self.v().block(s) != from && signal != self.v().get_bool(s, "powered") {
            if signal != self.v().get_bool(s, "open") {
                self.openable_sound(p, s, signal, None);
            }
            let ns = self.v().set_bool(self.v().set_bool(s, "powered", signal), "open", signal);
            self.set_block(p, ns, flag::CLIENTS);
        }
    }

    /// Opening or closing sound (door, trapdoor, fence gate; pitch 0.9 to 1.0 drawn on
    /// the world's random draw even when the sound is not sent) then `block_open` or
    /// `block_close` event attributed to `by`. The player acting by hand hears the prediction of their
    /// client: the server does not send it to them.
    pub(crate) fn openable_sound(&mut self, p: P, s: u32, open: bool, by: Option<crate::mobs::mob::Ent>) {
        let name = openable_sound_name(self.state_name(s), open);
        let pitch = self.mobs.rng.next_float() * 0.1 + 0.9;
        if by != Some(crate::mobs::mob::Ent::Player) {
            self.block_sound(p, &name, 1.0, pitch);
        }
        self.game_event_block(if open { "minecraft:block_open" } else { "minecraft:block_close" }, p, by, None);
    }

    pub(crate) fn trapdoor_neighbor_changed(&mut self, s: u32, p: P) {
        let signal = self.has_neighbor_signal(p);
        if signal != self.v().get_bool(s, "powered") {
            let mut ns = s;
            if self.v().get_bool(s, "open") != signal {
                ns = self.v().set_bool(ns, "open", signal);
                self.openable_sound(p, s, signal, None);
            }
            let ns = self.v().set_bool(ns, "powered", signal);
            self.set_block(p, ns, flag::CLIENTS);
            if self.v().get_bool(ns, "waterlogged") {
                self.schedule_fluid_tick(p, 1, 5);
            }
        }
    }

    pub(crate) fn gate_neighbor_changed(&mut self, s: u32, p: P) {
        let power = self.has_neighbor_signal(p);
        if self.v().get_bool(s, "powered") != power {
            let ns = self.v().set_bool(self.v().set_bool(s, "powered", power), "open", power);
            self.set_block(p, ns, flag::CLIENTS);
            if self.v().get_bool(s, "open") != power {
                self.openable_sound(p, s, power, None);
            }
        }
    }

    /// Opening by hand (door, trapdoor, fence gate): toggles `open`, sets flag 10 (2|8).
    pub(crate) fn openable_use(&mut self, p: P, s: u32) -> bool {
        let v = self.v();
        match v.kind(s) {
            Kind::Door { hand } | Kind::Trapdoor { hand } => {
                if !hand {
                    return false;
                }
                let ns = v.cycle(s, "open");
                self.set_block(p, ns, 10);
                if self.v().get_bool(ns, "waterlogged") {
                    self.schedule_fluid_tick(p, 1, 5);
                }
                let open = self.v().get_bool(ns, "open");
                self.openable_sound(p, s, open, Some(crate::mobs::mob::Ent::Player));
                true
            }
            Kind::FenceGate => {
                let mut ns = s;
                if v.get_bool(s, "open") {
                    ns = v.set_bool(ns, "open", false);
                } else {
                    let look = self.look_dir();
                    if v.facing(s) == look.opposite() {
                        ns = v.set_dir(ns, "facing", look);
                    }
                    ns = v.set_bool(ns, "open", true);
                }
                self.set_block(p, ns, 10);
                let open = self.v().get_bool(ns, "open");
                self.openable_sound(p, s, open, Some(crate::mobs::mob::Ent::Player));
                true
            }
            _ => false,
        }
    }

    // --- Dispensers, droppers, crafter (§ 3.13). ---

    pub(crate) fn dispenser_neighbor_changed(&mut self, s: u32, p: P) {
        let crafter = self.v().kind(s) == Kind::Crafter;
        let should = self.has_neighbor_signal(p) || (!crafter && self.has_neighbor_signal(above(p)));
        let triggered = self.v().get_bool(s, "triggered");
        let b = self.v().block(s);
        if should && !triggered {
            self.schedule_tick(p, b, 4);
            let ns = self.v().set_bool(s, "triggered", true);
            self.set_block(p, ns, flag::CLIENTS);
            if crafter {
                self.crafter_set_triggered(p, true); // G5-F11: signal seen by the menu
            }
        } else if !should && triggered {
            let mut ns = self.v().set_bool(s, "triggered", false);
            if crafter {
                ns = self.v().set_bool(ns, "crafting", false);
            }
            self.set_block(p, ns, flag::CLIENTS);
            if crafter {
                self.crafter_set_triggered(p, false);
            }
        }
    }

    pub(crate) fn dispenser_tick(&mut self, s: u32, p: P) {
        self.dispense_from(s, p);
    }

    // --- Hopper (§ 3.17), enabled state. ---

    pub(crate) fn hopper_check_powered(&mut self, p: P, s: u32) {
        let enabled = !self.has_neighbor_signal(p);
        if enabled != self.v().get_bool(s, "enabled") {
            let ns = self.v().set_bool(s, "enabled", enabled);
            self.set_block(p, ns, flag::CLIENTS);
        }
    }

    // --- Note block, copper bulb, target. ---

    pub(crate) fn note_neighbor_changed(&mut self, s: u32, p: P) {
        let signal = self.has_neighbor_signal(p);
        if signal != self.v().get_bool(s, "powered") {
            if signal {
                self.note_play(p, s, None);
            }
            let ns = self.v().set_bool(s, "powered", signal);
            self.set_update(p, ns);
        }
    }

    pub(crate) fn note_play(&mut self, p: P, s: u32, by: Option<crate::mobs::mob::Ent>) {
        let air_above = self.v().is_air(self.get(above(p)));
        let v = self.v();
        let inst = v.get(s, "instrument").unwrap_or("harp");
        let above_ok =
            matches!(inst, "zombie" | "skeleton" | "creeper" | "dragon" | "wither_skeleton" | "piglin" | "custom_head");
        if above_ok || air_above {
            let b = self.v().block(s);
            self.block_event(p, b, 0, 0);
            self.game_event_block("minecraft:note_block_play", p, by, None);
        }
    }

    /// Reception of the block event of a note: sound of the instrument (records category,
    /// volume 3, center of the block). Base instruments: pitch 2^((note − 12) / 12) (the note
    /// particle, note / 24, is the client's job); heads: pitch 1; player head: the sound
    /// of the head on top, and nothing (no event sent) if it has none.
    pub(crate) fn note_sound(&mut self, s: u32, p: P) -> bool {
        let v = self.v();
        let inst = v.get(s, "instrument").unwrap_or("harp");
        let (sound, pitch) = match inst {
            "custom_head" => {
                let custom = self.be_get(above(p)).and_then(|b| b.get("note_block_sound")?.as_str().map(str::to_owned));
                let Some(id) = custom else { return false };
                (rc_proto_v777::play::SoundRef::Named(id), 1.0)
            }
            "zombie" | "skeleton" | "creeper" | "dragon" | "wither_skeleton" | "piglin" => {
                let mob = if inst == "dragon" { "ender_dragon" } else { inst };
                let Some(id) = self.sound_id(&format!("minecraft:block.note_block.imitate.{mob}")) else { return false };
                (rc_proto_v777::play::SoundRef::Id(id), 1.0)
            }
            _ => {
                let note = v.get_int(s, "note");
                let pitch = 2f64.powf(f64::from(note - 12) / 12.0) as f32;
                let Some(id) = self.sound_id(&format!("minecraft:block.note_block.{inst}")) else { return false };
                (rc_proto_v777::play::SoundRef::Id(id), pitch)
            }
        };
        let seed = self.mobs.rng.next_long();
        let pos = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        let _ = self.send(rc_client::Event::Sound(rc_proto_v777::play::SoundPacket {
            sound,
            category: 2,
            pos: Some(pos),
            entity: None,
            volume: 3.0,
            pitch,
            seed,
        }));
        true
    }

    pub(crate) fn copper_bulb_check(&mut self, p: P, s: u32) {
        let signal = self.has_neighbor_signal(p);
        if signal != self.v().get_bool(s, "powered") {
            let mut ns = s;
            if !self.v().get_bool(s, "powered") {
                ns = self.v().cycle(ns, "lit");
                let on = self.v().get_bool(ns, "lit");
                let sound = if on { "minecraft:block.copper_bulb.turn_on" } else { "minecraft:block.copper_bulb.turn_off" };
                self.block_sound(p, sound, 1.0, 1.0);
            }
            let ns = self.v().set_bool(ns, "powered", signal);
            self.set_update(p, ns);
        }
    }

    pub(crate) fn target_tick(&mut self, s: u32, p: P) {
        if self.v().get_int(s, "power") != 0 {
            let ns = self.v().set_int(s, "power", 0);
            self.set_block(p, ns, flag::ALL);
        }
    }

    // --- Analog outputs (§ 3.5). ---

    pub(crate) fn analog_output(&self, s: u32, p: P, from: Dir) -> i32 {
        let v = self.v();
        // G6-F01: blocks with their own block entity.
        match v.kind(s) {
            Kind::Jukebox => return self.jukebox_comparator(p),
            Kind::Lectern => return self.lectern_comparator(p, s),
            Kind::Shelf => return self.shelf_comparator(p, s, from),
            Kind::ChiseledBookshelf => return self.bookshelf_comparator(p),
            Kind::DecoratedPot => return self.pot_comparator(p),
            _ => {}
        }
        let n = v.d.block_of(s).map_or("", |b| b.name.as_str());
        let n = n.strip_prefix("minecraft:").unwrap_or(n);
        match n {
            "water_cauldron" | "powder_snow_cauldron" => v.get_int(s, "level"),
            "lava_cauldron" => 3,
            "composter" => v.get_int(s, "level"),
            "cake" => (7 - v.get_int(s, "bites")) * 2,
            "beehive" | "bee_nest" => v.get_int(s, "honey_level"),
            "end_portal_frame" => {
                if v.get_bool(s, "eye") {
                    15
                } else {
                    0
                }
            }
            "respawn_anchor" => (v.get_int(s, "charges") as f32 / 4.0 * 15.0).floor() as i32,
            "creaking_heart" => self.heart_comparator(p), // F14
            "sculk_sensor" | "calibrated_sculk_sensor" => self.sensor_comparator(p, s), // G6-F10
            "detector_rail" => self.detector_analog(p, s), // G5-F06: minecart on top
            // G6-F01: copper golem statue, pose rank + 1.
            _ if n.ends_with("copper_golem_statue") => match v.get(s, "copper_golem_pose") {
                Some("sitting") => 2,
                Some("running") => 3,
                Some("star") => 4,
                _ => 1,
            },
            _ if n.ends_with("candle_cake") => 14, // colors and plain candle
            _ if n.ends_with("command_block") => self.be_int(p, "SuccessCount"), // S5
            _ if n.ends_with("copper_bulb") => {
                if v.get_bool(s, "lit") {
                    15
                } else {
                    0
                }
            }
            _ => self.container_fill(s, p),
        }
    }

    /// Decorated pot: filling of its single slot (key `item`).
    pub(crate) fn pot_comparator(&self, p: P) -> i32 {
        let Some(it) = self.be_get(p).and_then(|b| b.get("item")).and_then(crate::player::Stack::from_nbt) else {
            return 0;
        };
        // Stack size of the stack itself (component included), capped at 99 by the container.
        let r = it.count as f32 / self.life.items.max_stack(&it).min(99) as f32;
        if r <= 0.0 { 0 } else { 1 + (r * 14.0).floor() as i32 }
    }

    /// Filling of a container (double chest included): 0 empty, otherwise 1 + ⌊r × 14⌋.
    pub(crate) fn container_fill(&self, s: u32, p: P) -> i32 {
        let Some(size) = self.container_size(s) else { return 0 };
        // Blocked chest (conductor or sitting cat above a half): read as empty.
        let n = self.data.block_of(s).map_or("", |b| b.name.as_str());
        if (n == "minecraft:chest" || n == "minecraft:trapped_chest" || n.ends_with("copper_chest"))
            && self.chest_blocked(p)
        {
            return 0;
        }
        let mut parts: Vec<P> = vec![p];
        let mut total = size;
        if let Some(other) = self.chest_other_half(s, p) {
            // Left half first, like the game.
            if self.v().get(s, "type") == Some("right") {
                parts.insert(0, other);
            } else {
                parts.push(other);
            }
            total *= 2;
        }
        let mut r = 0.0f32;
        for q in parts {
            for (item, count, own) in self.be_item_counts_full(q) {
                // `min(99, stack size)`: `max_stack_size` component included.
                let max = own.unwrap_or_else(|| self.max_stack(&item)).min(99);
                r += count as f32 / max as f32;
            }
        }
        if r <= 0.0 {
            return 0;
        }
        r /= total as f32;
        1 + (r * 14.0).floor() as i32
    }
}

/// Sound family of a block set (wood, cherry, Nether, bamboo, copper, iron, stone).
fn block_set(name: &str) -> &'static str {
    let n = name.trim_start_matches("minecraft:");
    if n.contains("copper") {
        "copper"
    } else if n.starts_with("iron") || n.starts_with("stone") || n.starts_with("polished_blackstone") {
        "iron"
    } else if n.starts_with("cherry") {
        "cherry_wood"
    } else if n.starts_with("crimson") || n.starts_with("warped") {
        "nether_wood"
    } else if n.starts_with("bamboo") {
        "bamboo_wood"
    } else {
        "wooden"
    }
}

/// Sound of a pressure plate (types: metal for weighted plates, stone, wood).
pub fn plate_sound(name: &str, on: bool) -> String {
    let n = name.trim_start_matches("minecraft:");
    let set = if n.ends_with("weighted_pressure_plate") {
        "metal"
    } else if n.starts_with("stone") || n.starts_with("polished_blackstone") {
        "stone"
    } else {
        match block_set(name) {
            s @ ("cherry_wood" | "nether_wood" | "bamboo_wood") => s,
            _ => "wooden",
        }
    };
    format!("minecraft:block.{set}_pressure_plate.click_{}", if on { "on" } else { "off" })
}

/// Opening or closing sound of a door, trapdoor or fence gate.
pub fn openable_sound_name(name: &str, opening: bool) -> String {
    let set = block_set(name);
    let n = name.trim_start_matches("minecraft:");
    let what = if n.ends_with("_trapdoor") {
        "trapdoor"
    } else if n.ends_with("_fence_gate") {
        "fence_gate"
    } else {
        "door"
    };
    let set = match (what, set) {
        ("fence_gate", "wooden" | "copper" | "iron") => "",
        _ => set,
    };
    let sep = if set.is_empty() { "" } else { "_" };
    format!("minecraft:block.{set}{sep}{what}.{}", if opening { "open" } else { "close" })
}

/// Click of a button (pressed or released); stone, polished blackstone and metals: stone.
pub fn button_sound(name: &str, on: bool) -> String {
    let set = match block_set(name) {
        "iron" | "copper" => "stone",
        s => s,
    };
    format!("minecraft:block.{set}_button.click_{}", if on { "on" } else { "off" })
}

#[cfg(test)]
#[path = "redstone_tests.rs"]
mod tests;
