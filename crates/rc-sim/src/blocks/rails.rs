//! Rail shapes (§ 3.10): connections to neighbouring rails on placement and updates,
//! curves, slopes, straight rails (boosters, activators, detectors). Follows the game's
//! algorithm step by step, including its intermediate states (each reconnection places the neighbouring rail
//! with updates).

use super::{Kind, P, above, add, below};
use crate::server::Server;

/// Rail seen by the algorithm: position, state, straight or not, connections.
struct Rail {
    pos: P,
    state: u32,
    straight: bool,
    conns: Vec<P>,
}

fn conns_of(p: P, shape: &str) -> Vec<P> {
    let n = add(p, 0, 0, -1);
    let s = add(p, 0, 0, 1);
    let w = add(p, -1, 0, 0);
    let e = add(p, 1, 0, 0);
    match shape {
        "north_south" => vec![n, s],
        "east_west" => vec![w, e],
        "ascending_east" => vec![w, above(e)],
        "ascending_west" => vec![above(w), e],
        "ascending_north" => vec![above(n), s],
        "ascending_south" => vec![n, above(s)],
        "south_east" => vec![e, s],
        "south_west" => vec![w, s],
        "north_west" => vec![w, n],
        "north_east" => vec![e, n],
        _ => Vec::new(),
    }
}

impl Rail {
    fn has_conn(&self, q: P) -> bool {
        self.conns.iter().any(|c| c[0] == q[0] && c[2] == q[2])
    }

    fn connects_to(&self, other: &Rail) -> bool {
        self.has_conn(other.pos)
    }

    fn can_connect_to(&self, other: &Rail) -> bool {
        self.connects_to(other) || self.conns.len() != 2
    }
}

impl Server {
    /// Positions linked by the rail shape.
    pub(crate) fn rail_connections(&self, p: P, s: u32) -> Vec<P> {
        conns_of(p, self.v().get(s, "shape").unwrap_or("north_south"))
    }

    pub(crate) fn is_rail_state(&self, s: u32) -> bool {
        matches!(self.v().kind(s), Kind::Rail | Kind::PoweredRail | Kind::DetectorRail)
    }

    fn is_rail_at(&self, p: P) -> bool {
        self.is_rail_state(self.get(p))
    }

    fn rail_new(&self, p: P, s: u32) -> Rail {
        let v = self.v();
        let shape = v.get(s, "shape").unwrap_or("north_south");
        Rail { pos: p, state: s, straight: v.kind(s) != Kind::Rail, conns: conns_of(p, shape) }
    }

    /// Rail at `p`, above or below.
    fn rail_near(&self, p: P) -> Option<Rail> {
        for q in [p, above(p), below(p)] {
            let s = self.get(q);
            if self.is_rail_state(s) {
                return Some(self.rail_new(q, s));
            }
        }
        None
    }

    fn rail_remove_soft(&self, r: &mut Rail) {
        let mut i = 0;
        while i < r.conns.len() {
            match self.rail_near(r.conns[i]) {
                Some(o) if o.connects_to(r) => {
                    r.conns[i] = o.pos;
                    i += 1;
                }
                _ => {
                    r.conns.remove(i);
                }
            }
        }
    }

    fn rail_has_neighbor(&self, me: &Rail, q: P) -> bool {
        match self.rail_near(q) {
            Some(mut o) => {
                self.rail_remove_soft(&mut o);
                o.can_connect_to(me)
            }
            None => false,
        }
    }

    fn rail_potential(&self, p: P) -> usize {
        [add(p, 0, 0, -1), add(p, 1, 0, 0), add(p, 0, 0, 1), add(p, -1, 0, 0)]
            .iter()
            .filter(|&&q| self.is_rail_at(q) || self.is_rail_at(above(q)) || self.is_rail_at(below(q)))
            .count()
    }

    fn rail_slope(&self, p: P, shape: &'static str) -> &'static str {
        match shape {
            "north_south" => {
                let mut s = shape;
                if self.is_rail_at(above(add(p, 0, 0, -1))) {
                    s = "ascending_north";
                }
                if self.is_rail_at(above(add(p, 0, 0, 1))) {
                    s = "ascending_south";
                }
                s
            }
            "east_west" => {
                let mut s = shape;
                if self.is_rail_at(above(add(p, 1, 0, 0))) {
                    s = "ascending_east";
                }
                if self.is_rail_at(above(add(p, -1, 0, 0))) {
                    s = "ascending_west";
                }
                s
            }
            _ => shape,
        }
    }

    /// Rail `r` adds the connection toward `other` and places itself again.
    fn rail_connect_to(&mut self, r: &mut Rail, other: P) {
        r.conns.push(other);
        let p = r.pos;
        let n = r.has_conn(add(p, 0, 0, -1));
        let s = r.has_conn(add(p, 0, 0, 1));
        let w = r.has_conn(add(p, -1, 0, 0));
        let e = r.has_conn(add(p, 1, 0, 0));
        let mut shape: Option<&'static str> = None;
        if n || s {
            shape = Some("north_south");
        }
        if w || e {
            shape = Some("east_west");
        }
        if !r.straight {
            if s && e && !n && !w {
                shape = Some("south_east");
            }
            if s && w && !n && !e {
                shape = Some("south_west");
            }
            if n && w && !s && !e {
                shape = Some("north_west");
            }
            if n && e && !s && !w {
                shape = Some("north_east");
            }
        }
        let shape = self.rail_slope(p, shape.unwrap_or("north_south"));
        r.state = self.v().set(r.state, "shape", shape);
        self.set_update(p, r.state);
    }

    /// Shape from the neighbours, placement, then reconnection of the neighbours.
    fn rail_place(&mut self, mut me: Rail, signal: bool, first: bool, default: &'static str) -> u32 {
        let p = me.pos;
        let n = self.rail_has_neighbor(&me, add(p, 0, 0, -1));
        let s = self.rail_has_neighbor(&me, add(p, 0, 0, 1));
        let w = self.rail_has_neighbor(&me, add(p, -1, 0, 0));
        let e = self.rail_has_neighbor(&me, add(p, 1, 0, 0));
        let ns = n || s;
        let we = w || e;
        let mut shape: Option<&'static str> = None;
        if ns && !we {
            shape = Some("north_south");
        }
        if we && !ns {
            shape = Some("east_west");
        }
        let (se, sw, ne, nw) = (s && e, s && w, n && e, n && w);
        if !me.straight {
            if se && !n && !w {
                shape = Some("south_east");
            }
            if sw && !n && !e {
                shape = Some("south_west");
            }
            if nw && !s && !e {
                shape = Some("north_west");
            }
            if ne && !s && !w {
                shape = Some("north_east");
            }
        }
        if shape.is_none() {
            if ns && we {
                shape = Some(default);
            } else if ns {
                shape = Some("north_south");
            } else if we {
                shape = Some("east_west");
            }
            if !me.straight {
                // Order of the tests differs according to the signal: the last true one wins.
                let order: [(bool, &'static str); 4] = if signal {
                    [(se, "south_east"), (sw, "south_west"), (ne, "north_east"), (nw, "north_west")]
                } else {
                    [(nw, "north_west"), (ne, "north_east"), (sw, "south_west"), (se, "south_east")]
                };
                for (c, sh) in order {
                    if c {
                        shape = Some(sh);
                    }
                }
            }
        }
        let shape = match shape {
            Some(sh) => self.rail_slope(p, sh),
            None => default,
        };
        me.conns = conns_of(p, shape);
        me.state = self.v().set(me.state, "shape", shape);
        if first || self.get(p) != me.state {
            self.set_update(p, me.state);
            let mut i = 0;
            while i < me.conns.len() {
                if let Some(mut o) = self.rail_near(me.conns[i]) {
                    self.rail_remove_soft(&mut o);
                    if o.can_connect_to(&me) {
                        self.rail_connect_to(&mut o, me.pos);
                    }
                }
                i += 1;
            }
        }
        me.state
    }

    fn shape_name(&self, s: u32) -> &'static str {
        const ALL: [&str; 10] = [
            "north_south",
            "east_west",
            "ascending_east",
            "ascending_west",
            "ascending_north",
            "ascending_south",
            "south_east",
            "south_west",
            "north_west",
            "north_east",
        ];
        let v = self.v();
        let cur = v.get(s, "shape").unwrap_or("north_south");
        ALL.iter().find(|&&x| x == cur).copied().unwrap_or("north_south")
    }

    /// The direction update.
    pub(crate) fn rail_update_dir(&mut self, p: P, s: u32, first: bool) -> u32 {
        let current = self.shape_name(s);
        let signal = self.has_neighbor_signal(p);
        let me = self.rail_new(p, s);
        self.rail_place(me, signal, first, current)
    }

    /// Placing a rail (the reaction to a block placement → the state update): shape, then for straight rails a
    /// "neighbour changed" on itself (power).
    pub(crate) fn rail_placed(&mut self, p: P, s: u32, moved: bool) {
        let s = self.rail_update_dir(p, s, true);
        if self.v().kind(s) != Kind::Rail {
            let b = self.v().block(s);
            self.neighbor_changed_full(s, p, b, moved);
        }
    }

    /// Simple rail: a neighbour that is a signal source retriggers the shape if three rails surround it.
    pub(crate) fn plain_rail_update(&mut self, s: u32, p: P, from: u16) {
        let def = self.v().default_state(from);
        if self.is_signal_source(def) && self.rail_potential(p) == 3 {
            self.rail_update_dir(p, s, false);
        }
    }
}
