//! Pistons, part shared by the server and the client (§ 3.9): resolution of the pushed structure
//! (lines, sticky branches, limit of 12) on a read-only world, and client replay of the
//! block event. The server does not send the moving blocks: the client recreates them
//! by replaying the piston trigger without checking the signal, like the game.

use std::sync::Arc;

use rc_data::{VersionData, sim_bits};

use super::table::View;
use super::{Dir, Kind, P, Table, rel, rel_n};

/// World reading needed to resolve a structure.
pub trait PistonWorld {
    fn state(&self, p: [i32; 3]) -> u32;
    /// Lowest and highest (inclusive) block layers.
    fn min_y(&self) -> i32;
    fn max_y(&self) -> i32;
    /// Block inside the world border (low corner).
    fn in_border(&self, p: [i32; 3]) -> bool;
}

/// Result of resolving a structure.
pub(crate) struct Structure {
    pub push_dir: Dir,
    pub to_push: Vec<P>,
    pub to_destroy: Vec<P>,
}

/// Resolution of a structure on a world `W`.
pub(crate) struct Resolver<'a, W: PistonWorld + ?Sized> {
    pub v: &'a View,
    pub w: &'a W,
}

impl<W: PistonWorld + ?Sized> Resolver<'_, W> {
    /// Pushability (§ 3.9).
    pub fn pushable(&self, s: u32, q: P, d: Dir, allow_destroy: bool, connection: Dir) -> bool {
        let (min, max) = (self.w.min_y(), self.w.max_y());
        if q[1] < min || q[1] > max || !self.w.in_border(q) {
            return false;
        }
        let v = self.v;
        if v.is_air(s) {
            return true;
        }
        if (d == Dir::Down && q[1] == min) || (d == Dir::Up && q[1] == max) {
            return false;
        }
        if matches!(v.kind(s), Kind::Piston { .. }) {
            if v.get_bool(s, "extended") {
                return false;
            }
        } else {
            if v.hardness(s) == -1.0 {
                return false;
            }
            match v.push_reaction(s) {
                sim_bits::PUSH_BLOCK => return false,
                sim_bits::PUSH_DESTROY => return allow_destroy,
                sim_bits::PUSH_ONLY => return d == connection,
                _ => {}
            }
        }
        !v.has_block_entity(s)
    }

    fn sticky(&self, s: u32) -> bool {
        let b = self.v.block(s);
        b == self.v.t.id.slime || b == self.v.t.id.honey
    }

    fn stick_together(&self, a: u32, b: u32) -> bool {
        let (ba, bb) = (self.v.block(a), self.v.block(b));
        let id = &self.v.t.id;
        if (ba == id.honey && bb == id.slime) || (ba == id.slime && bb == id.honey) {
            return false;
        }
        self.sticky(a) || self.sticky(b)
    }

    pub fn resolve(&self, piston: P, dir: Dir, extending: bool) -> Option<Structure> {
        let (push_dir, start) =
            if extending { (dir, rel(piston, dir)) } else { (dir.opposite(), rel_n(piston, dir, 2)) };
        let mut st = Structure { push_dir, to_push: Vec::new(), to_destroy: Vec::new() };
        let ns = self.w.state(start);
        if !self.pushable(ns, start, push_dir, false, dir) {
            if extending && self.v.push_reaction(ns) == sim_bits::PUSH_DESTROY {
                st.to_destroy.push(start);
                return Some(st);
            }
            return None;
        }
        if !self.add_line(&mut st, piston, start, push_dir) {
            return None;
        }
        let mut i = 0;
        while i < st.to_push.len() {
            let q = st.to_push[i];
            if self.sticky(self.w.state(q)) && !self.add_branches(&mut st, piston, q) {
                return None;
            }
            i += 1;
        }
        Some(st)
    }

    fn add_line(&self, st: &mut Structure, piston: P, start: P, direction: Dir) -> bool {
        let pd = st.push_dir;
        let mut next = self.w.state(start);
        if self.v.is_air(next) {
            return true;
        }
        if !self.pushable(next, start, pd, false, direction) || start == piston || st.to_push.contains(&start) {
            return true;
        }
        let mut count = 1;
        if count + st.to_push.len() > 12 {
            return false;
        }
        while self.sticky(next) {
            let q = rel_n(start, pd.opposite(), count as i32);
            let prev = next;
            next = self.w.state(q);
            if self.v.is_air(next)
                || !self.stick_together(prev, next)
                || !self.pushable(next, q, pd, false, pd.opposite())
                || q == piston
            {
                break;
            }
            count += 1;
            if count + st.to_push.len() > 12 {
                return false;
            }
        }
        let mut added = 0;
        for i in (0..count).rev() {
            st.to_push.push(rel_n(start, pd.opposite(), i as i32));
            added += 1;
        }
        let mut i = 1;
        loop {
            let q = rel_n(start, pd, i);
            if let Some(col) = st.to_push.iter().position(|&x| x == q) {
                // Reordering: the current line goes just before the block hit.
                let len = st.to_push.len();
                let head: Vec<P> = st.to_push[..col].to_vec();
                let last: Vec<P> = st.to_push[len - added..].to_vec();
                let mid: Vec<P> = st.to_push[col..len - added].to_vec();
                st.to_push = head.into_iter().chain(last).chain(mid).collect();
                for j in 0..=col + added {
                    let b = st.to_push[j];
                    if self.sticky(self.w.state(b)) && !self.add_branches(st, piston, b) {
                        return false;
                    }
                }
                return true;
            }
            next = self.w.state(q);
            if self.v.is_air(next) {
                return true;
            }
            if !self.pushable(next, q, pd, true, pd) || q == piston {
                return false;
            }
            if self.v.push_reaction(next) == sim_bits::PUSH_DESTROY {
                st.to_destroy.push(q);
                return true;
            }
            if st.to_push.len() >= 12 {
                return false;
            }
            st.to_push.push(q);
            added += 1;
            i += 1;
        }
    }

    fn add_branches(&self, st: &mut Structure, piston: P, from: P) -> bool {
        let fs = self.w.state(from);
        for d in Dir::ALL {
            if d.axis() != st.push_dir.axis() {
                let q = rel(from, d);
                let qs = self.w.state(q);
                if self.stick_together(qs, fs) && !self.add_line(st, piston, q, d) {
                    return false;
                }
            }
        }
        true
    }
}

/// Moving block seen by the client (block entity `minecraft:piston`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MovingBlock {
    /// Moved state (piston head for the source that extends, base for the one that retracts).
    pub moved: u32,
    /// Piston direction (0 down, 1 up, 2 north, 3 south, 4 west, 5 east).
    pub dir: u8,
    pub extending: bool,
    pub source: bool,
}

/// Client world of the replay: existing blocks and moving blocks.
pub trait ReplayWorld: PistonWorld {
    fn mover(&self, p: [i32; 3]) -> Option<MovingBlock>;
}

/// Local effect of the replay, in game order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayOp {
    /// Block placed locally (the block entity of a replaced block disappears with it).
    Set([i32; 3], u32),
    /// Moving block created at this position (placed just before), progress 0.
    Mover([i32; 3], MovingBlock),
    /// Crushed block (earlier state): local break effect (event 2001), except fire.
    Destroyed([i32; 3], u32),
}

/// Client replay of the block event of a piston.
pub struct PistonReplay {
    v: View,
    fire: [u16; 2],
}

/// World seen by the replay: the client world plus the changes already made.
struct Overlay<'a> {
    w: &'a dyn ReplayWorld,
    sets: Vec<(P, u32)>,
    movers: Vec<(P, Option<MovingBlock>)>,
}

impl PistonWorld for Overlay<'_> {
    fn state(&self, p: [i32; 3]) -> u32 {
        self.sets.iter().rev().find(|e| e.0 == p).map_or_else(|| self.w.state(p), |e| e.1)
    }
    fn min_y(&self) -> i32 {
        self.w.min_y()
    }
    fn max_y(&self) -> i32 {
        self.w.max_y()
    }
    fn in_border(&self, p: [i32; 3]) -> bool {
        self.w.in_border(p)
    }
}

impl Overlay<'_> {
    fn mover(&self, p: P) -> Option<MovingBlock> {
        self.movers.iter().rev().find(|e| e.0 == p).map_or_else(|| self.w.mover(p), |e| e.1)
    }
}

/// Changes of the replay and what they become for the client.
struct Plan<'a> {
    o: Overlay<'a>,
    ops: Vec<ReplayOp>,
}

impl Plan<'_> {
    fn set(&mut self, p: P, s: u32) {
        self.o.sets.push((p, s));
        self.o.movers.push((p, None));
        self.ops.push(ReplayOp::Set(p, s));
    }

    fn mover(&mut self, p: P, m: MovingBlock) {
        self.o.movers.push((p, Some(m)));
        self.ops.push(ReplayOp::Mover(p, m));
    }
}

impl PistonReplay {
    pub fn new(data: Arc<VersionData>) -> Self {
        let t = Arc::new(Table::new(&data));
        let fire = [t.id.fire, t.id.soul_fire];
        PistonReplay { v: View { t, d: data }, fire }
    }

    /// The block is a piston (normal or sticky): its event is replayed.
    pub fn is_piston(&self, s: u32) -> bool {
        matches!(self.v.kind(s), Kind::Piston { .. })
    }

    /// Local effects of the event (`b0` 0 extend, 1 retract, 2 retract without pulling; `b1`
    /// direction of the base) of the piston at `pos`; empty if it is not a piston or if the
    /// structure does not move.
    pub fn replay(&self, w: &dyn ReplayWorld, pos: [i32; 3], b0: u8, b1: u8) -> Vec<ReplayOp> {
        let s = w.state(pos);
        let Kind::Piston { sticky } = self.v.kind(s) else { return Vec::new() };
        let v = &self.v;
        let dir = v.facing(s);
        let extended = v.set_bool(s, "extended", true);
        let mut plan = Plan { o: Overlay { w, sets: Vec::new(), movers: Vec::new() }, ops: Vec::new() };
        if b0 == 0 {
            if !self.move_blocks(&mut plan, pos, dir, true, sticky) {
                return Vec::new();
            }
            plan.set(pos, extended);
        } else if b0 == 1 || b0 == 2 {
            let head = rel(pos, dir);
            if plan.o.mover(head).is_some() {
                self.final_tick(&mut plan, head);
            }
            let mp = self.moving_state(dir, sticky);
            plan.set(pos, mp);
            let base =
                v.set_dir(v.default_state(self.base_block(sticky)), "facing", Dir::from_index(usize::from(b1 & 7)));
            plan.mover(pos, MovingBlock { moved: base, dir: dir as u8, extending: false, source: true });
            if sticky {
                let two = rel_n(pos, dir, 2);
                let ms = plan.o.state(two);
                let mut piece = false;
                if v.kind(ms) == Kind::MovingPiston
                    && let Some(m) = plan.o.mover(two)
                    && m.dir == dir as u8
                    && m.extending
                {
                    self.final_tick(&mut plan, two);
                    piece = true;
                }
                if !piece {
                    let r = Resolver { v, w: &plan.o };
                    let pull_ok = b0 == 1
                        && !v.is_air(ms)
                        && r.pushable(ms, two, dir.opposite(), false, dir)
                        && (v.push_reaction(ms) == sim_bits::PUSH_NORMAL || matches!(v.kind(ms), Kind::Piston { .. }));
                    if pull_ok {
                        self.move_blocks(&mut plan, pos, dir, false, sticky);
                    } else {
                        plan.set(head, self.air());
                    }
                }
            } else {
                plan.set(head, self.air());
            }
        }
        plan.ops
    }

    fn air(&self) -> u32 {
        self.v.default_state(self.v.t.id.air)
    }

    fn base_block(&self, sticky: bool) -> u16 {
        if sticky { self.v.t.id.sticky_piston } else { self.v.t.id.piston }
    }

    fn moving_state(&self, dir: Dir, sticky: bool) -> u32 {
        let v = &self.v;
        let s = v.set_dir(v.default_state(v.t.id.moving_piston), "facing", dir);
        v.set(s, "type", if sticky { "sticky" } else { "normal" })
    }

    /// Forced end on the client side: air for the source, otherwise the moved state.
    fn final_tick(&self, plan: &mut Plan<'_>, p: P) {
        let Some(m) = plan.o.mover(p) else { return };
        plan.o.movers.push((p, None));
        if self.v.kind(plan.o.state(p)) == Kind::MovingPiston {
            let ns = if m.source { self.air() } else { m.moved };
            plan.set(p, ns);
        }
    }

    fn move_blocks(&self, plan: &mut Plan<'_>, piston: P, dir: Dir, extending: bool, sticky: bool) -> bool {
        let v = &self.v;
        let arm = rel(piston, dir);
        if !extending && v.kind(plan.o.state(arm)) == Kind::PistonHead {
            plan.set(arm, self.air());
        }
        let Some(st) = (Resolver { v, w: &plan.o }).resolve(piston, dir, extending) else { return false };
        let shapes: Vec<u32> = st.to_push.iter().map(|&q| plan.o.state(q)).collect();
        let mut delete: Vec<P> = st.to_push.clone();
        let push_dir = if extending { dir } else { dir.opposite() };
        let air = self.air();
        for &q in st.to_destroy.iter().rev() {
            let s = plan.o.state(q);
            if !self.fire.contains(&v.block(s)) {
                plan.ops.push(ReplayOp::Destroyed(q, s));
            }
            plan.set(q, air);
        }
        let mp = self.moving_state(dir, false);
        for i in (0..st.to_push.len()).rev() {
            let t = rel(st.to_push[i], push_dir);
            delete.retain(|&x| x != t);
            plan.set(t, mp);
            plan.mover(t, MovingBlock { moved: shapes[i], dir: dir as u8, extending, source: false });
        }
        if extending {
            let head = v.set_dir(v.default_state(v.t.id.piston_head), "facing", dir);
            let head = v.set(head, "type", if sticky { "sticky" } else { "normal" });
            delete.retain(|&x| x != arm);
            plan.set(arm, self.moving_state(dir, sticky));
            plan.mover(arm, MovingBlock { moved: head, dir: dir as u8, extending: true, source: true });
        }
        for q in delete {
            plan.set(q, air);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hashbrown::HashMap;

    struct Map {
        blocks: HashMap<P, u32>,
        movers: HashMap<P, MovingBlock>,
        air: u32,
    }

    impl PistonWorld for Map {
        fn state(&self, p: [i32; 3]) -> u32 {
            self.blocks.get(&p).copied().unwrap_or(self.air)
        }
        fn min_y(&self) -> i32 {
            -64
        }
        fn max_y(&self) -> i32 {
            319
        }
        fn in_border(&self, _p: [i32; 3]) -> bool {
            true
        }
    }

    impl ReplayWorld for Map {
        fn mover(&self, p: [i32; 3]) -> Option<MovingBlock> {
            self.movers.get(&p).copied()
        }
    }

    /// State `name[key=value,…]` (missing properties: default values).
    pub(crate) fn parse(v: &View, n: &str) -> u32 {
        let (name, props) = n.split_once('[').map_or((n, ""), |(a, b)| (a, b.trim_end_matches(']')));
        let b = v.t.block_id(name);
        assert_ne!(b, u16::MAX, "block {name}");
        let mut s = v.default_state(b);
        for kv in props.split(',').filter(|x| !x.is_empty()) {
            let (k, val) = kv.split_once('=').expect("key=value");
            let ns = v.set(s, k, val);
            assert!(ns != s || v.get(s, k) == Some(val), "property {kv} of {name}");
            s = ns;
        }
        s
    }

    fn setup() -> (PistonReplay, Map, impl Fn(&str) -> u32) {
        let data = Arc::new(rc_testdata::version_data().expect("version data missing (extraction)"));
        let r = PistonReplay::new(data);
        let v = View { t: r.v.t.clone(), d: r.v.d.clone() };
        let st = move |n: &str| parse(&v, n);
        let air = st("minecraft:air");
        (r, Map { blocks: HashMap::new(), movers: HashMap::new(), air }, st)
    }

    /// Extension to the east of a piston in front of stone: the stone and the head become
    /// moving blocks, the base becomes extended (game order: stone, head, base).
    #[test]
    fn extend_replays_moving_blocks() {
        let (r, mut w, st) = setup();
        let piston = st("minecraft:piston[facing=east,extended=false]");
        let stone = st("minecraft:stone");
        w.blocks.insert([0, 64, 0], piston);
        w.blocks.insert([1, 64, 0], stone);
        let ops = r.replay(&w, [0, 64, 0], 0, 5);
        let mp = st("minecraft:moving_piston[facing=east,type=normal]");
        let head = st("minecraft:piston_head[facing=east,type=normal,short=false]");
        assert_eq!(
            ops,
            vec![
                ReplayOp::Set([2, 64, 0], mp),
                ReplayOp::Mover([2, 64, 0], MovingBlock { moved: stone, dir: 5, extending: true, source: false }),
                ReplayOp::Set([1, 64, 0], mp),
                ReplayOp::Mover([1, 64, 0], MovingBlock { moved: head, dir: 5, extending: true, source: true }),
                ReplayOp::Set([0, 64, 0], st("minecraft:piston[facing=east,extended=true]")),
            ]
        );
    }

    /// Bench: construction (client start) and replay of an extension that pushes 12 blocks.
    #[test]
    #[ignore]
    fn bench_replay() {
        let data = Arc::new(rc_testdata::version_data().expect("version data missing (extraction)"));
        let t0 = std::time::Instant::now();
        let r = PistonReplay::new(data.clone());
        let built = t0.elapsed();
        let v = View { t: r.v.t.clone(), d: r.v.d.clone() };
        let air = parse(&v, "minecraft:air");
        let mut w = Map { blocks: HashMap::new(), movers: HashMap::new(), air };
        w.blocks.insert([0, 64, 0], parse(&v, "minecraft:piston[facing=east,extended=false]"));
        for x in 1..=12 {
            w.blocks.insert([x, 64, 0], parse(&v, "minecraft:stone"));
        }
        let n = 10_000;
        let t1 = std::time::Instant::now();
        let mut ops = 0;
        for _ in 0..n {
            ops += std::hint::black_box(r.replay(&w, [0, 64, 0], 0, 5)).len();
        }
        let per = t1.elapsed() / n;
        println!("build {built:?}; replay 12 blocks {per:?} ({} operations)", ops / n as usize);
    }

    /// Retraction of a sticky piston that pulls the stone; the block crushed by an extension
    /// produces the break effect; an immobile block is not replayed.
    #[test]
    fn retract_pull_and_destroy() {
        let (r, mut w, st) = setup();
        let air = st("minecraft:air");
        let sticky = st("minecraft:sticky_piston[facing=up,extended=true]");
        let head = st("minecraft:piston_head[facing=up,type=sticky,short=false]");
        let stone = st("minecraft:stone");
        w.blocks.insert([0, 64, 0], sticky);
        w.blocks.insert([0, 65, 0], head);
        w.blocks.insert([0, 66, 0], stone);
        let ops = r.replay(&w, [0, 64, 0], 1, 1);
        let base = st("minecraft:sticky_piston[facing=up,extended=false]");
        let mp_sticky = st("minecraft:moving_piston[facing=up,type=sticky]");
        let mp = st("minecraft:moving_piston[facing=up,type=normal]");
        assert_eq!(
            ops,
            vec![
                ReplayOp::Set([0, 64, 0], mp_sticky),
                ReplayOp::Mover([0, 64, 0], MovingBlock { moved: base, dir: 1, extending: false, source: true }),
                ReplayOp::Set([0, 65, 0], air),
                ReplayOp::Set([0, 65, 0], mp),
                ReplayOp::Mover([0, 65, 0], MovingBlock { moved: stone, dir: 1, extending: false, source: false }),
                ReplayOp::Set([0, 66, 0], air),
            ]
        );
        // Extension that crushes a torch.
        let mut w2 = Map { blocks: HashMap::new(), movers: HashMap::new(), air };
        w2.blocks.insert([0, 64, 0], st("minecraft:piston[facing=east,extended=false]"));
        let torch = st("minecraft:torch");
        w2.blocks.insert([1, 64, 0], torch);
        let ops = r.replay(&w2, [0, 64, 0], 0, 5);
        assert_eq!(ops[0], ReplayOp::Destroyed([1, 64, 0], torch));
        assert_eq!(ops[1], ReplayOp::Set([1, 64, 0], air));
        // Obsidian: nothing moves.
        w2.blocks.insert([1, 64, 0], st("minecraft:obsidian"));
        assert!(r.replay(&w2, [0, 64, 0], 0, 5).is_empty());
        assert!(r.replay(&w2, [1, 64, 0], 0, 5).is_empty());
    }
}
