//! Creaking heart (block entity, `server-mobs.md` § 4.F14): tick (comparator signal,
//! distress call when its creaking is hit, awake or dormant state according to the
//! timeline, protector spawning at night near a player, sending back a protector too
//! far or stuck), UUID link kept in the NBT (`creaking`), resin sown on distress.
//! The trail particles (heart ↔ creaking) are not sent (the client's particle format
//! has no destination) but their random draws are made.

use hashbrown::HashMap;

use rc_proto::nbt::Nbt;

use super::super::mob::{Ent, Mob};
use super::super::species::Kind;
use crate::nbtio;
use crate::server::Server;

/// Link to the protector: network id (loaded mob) or only the UUID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    pub uuid: u128,
    pub id: Option<i32>,
}

/// Run state of a heart (the rest is in the block entity's NBT).
#[derive(Debug, Clone, Default)]
pub struct HeartState {
    pub link: Option<Link>,
    pub ticks_existed: i64,
    pub ticker: i32,
    pub emitter: i32,
    pub emitter_target: Option<[f64; 3]>,
    pub output: i32,
    /// Block entity ticker id.
    pub ticker_id: u64,
}

/// Known hearts (position → state).
#[derive(Debug, Clone, Default)]
pub struct Hearts {
    pub map: HashMap<[i32; 3], HeartState>,
}

pub fn is_heart_block(name: &str) -> bool {
    name == "minecraft:creaking_heart"
}

fn center(p: [i32; 3]) -> [f64; 3] {
    [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5]
}

fn bbox_center(m: &Mob) -> [f64; 3] {
    let b = m.bbox();
    [(b.min[0] + b.max[0]) * 0.5, (b.min[1] + b.max[1]) * 0.5, (b.min[2] + b.max[2]) * 0.5]
}

fn uuid_ints(u: u128) -> Vec<i32> {
    vec![(u >> 96) as i32, (u >> 64) as i32, (u >> 32) as i32, u as i32]
}

fn ints_uuid(v: &[i32]) -> u128 {
    v.iter().fold(0u128, |a, &x| (a << 32) | u128::from(x as u32))
}

impl Server {
    /// Value of the heart's state (`creaking_heart_state`): `uprooted`, `dormant` or `awake`.
    pub(crate) fn heart_state_value(&self, p: [i32; 3]) -> Option<String> {
        if !is_heart_block(self.bname(p)) {
            return None;
        }
        self.prop(p, "creaking_heart_state")
    }

    /// Heart ticker (created if the heart is not uprooted).
    pub(crate) fn heart_register(&mut self, p: [i32; 3]) {
        if self.heart_state_value(p).is_none_or(|v| v == "uprooted") {
            return;
        }
        if let Some(h) = self.mobs.rec.hearts.map.get(&p)
            && h.ticker_id != 0
            && self.bs.tickers.iter().chain(self.bs.pending_tickers.iter()).any(|t| *t == (h.ticker_id, p))
        {
            return;
        }
        let id = self.add_ticker(p);
        let link = self.block_entity(p).and_then(|be| match be.get("creaking") {
            Some(Nbt::IntArray(v)) if v.len() == 4 => Some(Link { uuid: ints_uuid(v), id: None }),
            _ => None,
        });
        let e = self.mobs.rec.hearts.map.entry(p).or_default();
        e.ticker_id = id;
        if e.link.is_none() {
            e.link = link;
        }
    }

    pub(crate) fn heart_is_ticker(&self, id: u64, p: [i32; 3]) -> bool {
        self.mobs.rec.hearts.map.get(&p).is_some_and(|h| h.ticker_id == id && id != 0)
    }

    /// Ticker still valid: heart present and not uprooted.
    pub(crate) fn heart_ticker_alive(&self, id: u64, p: [i32; 3]) -> bool {
        self.heart_is_ticker(id, p) && self.heart_state_value(p).is_some_and(|v| v != "uprooted")
    }

    /// NBT link updated (`creaking`: protector's UUID).
    fn heart_store(&mut self, p: [i32; 3]) {
        let link = self.mobs.rec.hearts.map.get(&p).and_then(|h| h.link);
        if let Some(be) = self.be_get_mut(p) {
            match link {
                Some(l) => nbtio::set(be, "creaking", Nbt::IntArray(uuid_ints(l.uuid))),
                None => nbtio::remove(be, "creaking"),
            }
        }
    }

    /// Mob loaded by id, otherwise by UUID; link forgotten
    /// after 30 ticks of existence without finding it.
    pub(crate) fn heart_protector(&mut self, p: [i32; 3]) -> Option<i32> {
        let link = self.mobs.rec.hearts.map.get(&p)?.link?;
        if let Some(id) = link.id
            && self.mob_ref(id).is_some_and(|m| !m.removed)
        {
            return Some(id);
        }
        if let Some(m) = self.mobs.find_uuid_where(link.uuid, |m| !m.removed && m.kind == Kind::Creaking) {
            let id = m.id;
            if let Some(h) = self.mobs.rec.hearts.map.get_mut(&p) {
                h.link = Some(Link { uuid: link.uuid, id: Some(id) });
            }
            return Some(id);
        }
        // Mob not found (removed, or not yet loaded).
        if let Some(h) = self.mobs.rec.hearts.map.get_mut(&p) {
            h.link = Some(Link { uuid: link.uuid, id: None });
            if h.ticks_existed >= 30 {
                h.link = None;
                self.heart_store(p);
            }
        }
        None
    }

    /// Does the heart protect this mob? (Mob being ticked: taken
    /// by its id, the list does not contain it.)
    pub(crate) fn heart_is_protector(&mut self, p: [i32; 3], m: &Mob) -> bool {
        if !is_heart_block(self.bname(p)) || self.block_entity(p).is_none() {
            return false;
        }
        let Some(link) = self.mobs.rec.hearts.map.get(&p).and_then(|h| h.link) else {
            // Link kept only in the NBT (heart not yet ticked): read back.
            let uuid = self.block_entity(p).and_then(|be| match be.get("creaking") {
                Some(Nbt::IntArray(v)) if v.len() == 4 => Some(ints_uuid(v)),
                _ => None,
            });
            return uuid == Some(m.uuid);
        };
        if link.uuid != m.uuid {
            return false;
        }
        if let Some(h) = self.mobs.rec.hearts.map.get_mut(&p) {
            h.link = Some(Link { uuid: m.uuid, id: Some(m.id) });
        }
        true
    }

    /// The heart's server tick.
    pub(crate) fn heart_tick(&mut self, p: [i32; 3]) {
        let Some(h) = self.mobs.rec.hearts.map.get_mut(&p) else { return };
        h.ticks_existed += 1;
        // Comparator signal.
        let computed = self.heart_output(p);
        if self.mobs.rec.hearts.map.get(&p).is_some_and(|h| h.output != computed) {
            if let Some(h) = self.mobs.rec.hearts.map.get_mut(&p) {
                h.output = computed;
            }
            let block = self.v().block(self.st(p));
            self.update_neighbour_for_output_signal(p, block);
        }
        // Distress call (100 ticks).
        let emitter = self.mobs.rec.hearts.map.get(&p).map_or(0, |h| h.emitter);
        if emitter > 0 {
            if emitter > 50 {
                self.heart_emit_particles(p, 1);
                self.heart_emit_particles(p, 1);
            }
            let target = self.mobs.rec.hearts.map.get(&p).and_then(|h| h.emitter_target);
            if emitter % 10 == 0
                && let Some(mut target) = target
            {
                if let Some(id) = self.heart_protector(p)
                    && let Some(c) = self.mob_ref(id)
                {
                    target = bbox_center(c);
                    if let Some(h) = self.mobs.rec.hearts.map.get_mut(&p) {
                        h.emitter_target = Some(target);
                    }
                }
                let hp = center(p);
                let progress = 0.2 + 0.8 * (100 - emitter) as f32 / 100.0;
                let loc: [f64; 3] = std::array::from_fn(|i| (hp[i] - target[i]) * f64::from(progress) + target[i]);
                let bp = loc.map(|v| v.floor() as i32);
                let volume = emitter as f32 / 2.0 / 100.0 + 0.5;
                self.block_sound(bp, "minecraft:block.creaking_heart.hurt", volume, 1.0);
            }
            if let Some(h) = self.mobs.rec.hearts.map.get_mut(&p) {
                h.emitter -= 1;
            }
        }
        let ticker = self.mobs.rec.hearts.map.get(&p).map_or(0, |h| h.ticker);
        if let Some(h) = self.mobs.rec.hearts.map.get_mut(&p) {
            h.ticker -= 1;
        }
        if ticker >= 0 {
            return;
        }
        let next = self.mobs.rng.next_int_bound(5) + 20;
        if let Some(h) = self.mobs.rec.hearts.map.get_mut(&p) {
            h.ticker = next;
        }
        let state = self.st(p);
        let updated = self.heart_updated_state(p, state);
        if updated != state {
            self.set_update(p, updated);
            if self.heart_state_value(p).as_deref() == Some("uprooted") {
                return;
            }
        }
        let linked = self.mobs.rec.hearts.map.get(&p).is_some_and(|h| h.link.is_some());
        if !linked {
            if self.heart_state_value(p).as_deref() == Some("awake")
                && self.rule("minecraft:spawn_monsters", true)
                && self.level.difficulty_id() != 0
                && self.game_mode != 3
                && super::super::mth::dist2(self.pos, [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])])
                    <= 32.0 * 32.0
                && let Some(id) = self.heart_spawn_protector(p)
            {
                if let Some(i) = self.mob_index(id)
                    && let Some(mut c) = self.mobs.list[i].take()
                {
                    let uuid = c.uuid;
                    if let Some(h) = self.mobs.rec.hearts.map.get_mut(&p) {
                        h.link = Some(Link { uuid, id: Some(id) });
                    }
                    self.heart_store(p);
                    self.make_sound(&mut c, "minecraft:entity.creaking.spawn");
                    self.mobs.list[i] = Some(c);
                }
                self.block_sound(p, "minecraft:block.creaking_heart.spawn", 1.0, 1.0);
            }
        } else if let Some(id) = self.heart_protector(p)
            && let Some(i) = self.mob_index(id)
            && let Some(mut c) = self.mobs.list[i].take()
        {
            let dist = {
                let b = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
                super::super::mth::dist2(c.pos, b).sqrt()
            };
            let remove = (!super::creaking_active(self) && !c.persistent)
                || dist > 34.0
                || super::creaking::player_stuck_in_you(self, &mut c);
            self.mobs.list[i] = Some(c);
            if remove {
                self.heart_remove_protector(p, None);
            }
        }
    }

    /// Uprooted without the required logs (and without protector), otherwise
    /// awake or dormant according to the timeline.
    fn heart_updated_state(&self, p: [i32; 3], state: u32) -> u32 {
        let linked = self.mobs.rec.hearts.map.get(&p).is_some_and(|h| h.link.is_some());
        let v = if !self.heart_has_logs(p, state) && !linked {
            "uprooted"
        } else if super::creaking_active(self) {
            "awake"
        } else {
            "dormant"
        };
        self.with_prop(state, "creaking_heart_state", v).unwrap_or(state)
    }

    /// Pale oak logs of the same axis on both sides of the heart's axis.
    pub(crate) fn heart_has_logs(&self, p: [i32; 3], state: u32) -> bool {
        let axis = self.data.state_properties(state).find(|(k, _)| *k == "axis").map_or("y", |(_, v)| v);
        let dirs: [[i32; 3]; 2] = match axis {
            "x" => [[-1, 0, 0], [1, 0, 0]],
            "z" => [[0, 0, -1], [0, 0, 1]],
            _ => [[0, -1, 0], [0, 1, 0]],
        };
        dirs.iter().all(|d| {
            let q = [p[0] + d[0], p[1] + d[1], p[2] + d[2]];
            self.btag_id(q, crate::tag_bits::TagId::PaleOakLogs) && self.prop(q, "axis").as_deref() == Some(axis)
        })
    }

    /// Scheduled block tick (after the shape update): an uprooted heart that finds its logs again
    /// wakes up or goes dormant. True if the block is a heart.
    pub(crate) fn heart_block_tick(&mut self, state: u32, p: [i32; 3]) -> bool {
        if !is_heart_block(self.state_name(state)) {
            return false;
        }
        let uprooted = self.data.state_properties(state).any(|(k, v)| k == "creaking_heart_state" && v == "uprooted");
        if self.heart_has_logs(p, state) && uprooted {
            let v = if super::creaking_active(self) { "awake" } else { "dormant" };
            if let Some(n) = self.with_prop(state, "creaking_heart_state", v) {
                self.set_update(p, n);
                self.heart_register(p);
            }
        }
        true
    }

    /// Distance from the protector to the bottom of the heart.
    fn heart_distance(&mut self, p: [i32; 3]) -> f64 {
        let b = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
        self.heart_protector(p)
            .and_then(|id| self.mob_ref(id))
            .map_or(0.0, |c| super::super::mth::dist2(c.pos, b).sqrt())
    }

    /// 15 near the heart, 0 at 32 blocks or without a protector.
    fn heart_output(&mut self, p: [i32; 3]) -> i32 {
        if self.mobs.rec.hearts.map.get(&p).is_none_or(|h| h.link.is_none()) || self.heart_protector(p).is_none() {
            return 0;
        }
        let d = self.heart_distance(p).clamp(0.0, 32.0) / 32.0;
        15 - (d * 15.0).floor() as i32
    }

    /// Comparator signal (0 if uprooted).
    pub(crate) fn heart_comparator(&self, p: [i32; 3]) -> i32 {
        if self.heart_state_value(p).as_deref() == Some("uprooted") {
            return 0;
        }
        self.mobs.rec.hearts.map.get(&p).map_or(0, |h| h.output)
    }

    /// Spawn by tries (creaking, spawner, 5 tries, 16/8, on a
    /// support without leaves, collisions checked), then link to the heart. Event 60
    /// (puff) goes out before the client knows the mob: it reaches nobody.
    fn heart_spawn_protector(&mut self, p: [i32; 3]) -> Option<i32> {
        let id = self.spawn_util(Kind::Creaking, super::super::spawn::Reason::Spawner, p, 5, 16, 8, true, true)?;
        if let Some(i) = self.mob_index(id)
            && let Some(mut c) = self.mobs.list[i].take()
        {
            let pos = c.pos;
            self.game_event_at("minecraft:entity_place", pos, Some(Ent::Mob(c.id)));
            super::creaking::set_transient(&mut c, p);
            self.mobs.list[i] = Some(c);
        }
        Some(id)
    }

    /// Distress (particles, resin sown on the neighboring logs if awake).
    /// The hurt creaking is out of the list (being processed): it is passed here.
    pub(crate) fn heart_creaking_hurt(&mut self, p: [i32; 3], c: &Mob) {
        if !self.heart_is_protector(p, c) {
            return;
        }
        if self.mobs.rec.hearts.map.get(&p).is_some_and(|h| h.emitter > 0) {
            return;
        }
        self.heart_particle_draws(20);
        if self.heart_state_value(p).as_deref() == Some("awake") {
            let n = self.mobs.rng.int_between(2, 3);
            for _ in 0..n {
                if let Some(q) = self.heart_spread_resin(p) {
                    self.block_sound(q, "minecraft:block.resin.place", 1.0, 1.0);
                    let st = self.st(p);
                    self.game_event_block("minecraft:block_place", q, None, Some(st));
                }
            }
        }
        let target = bbox_center(c);
        if let Some(h) = self.mobs.rec.hearts.map.get_mut(&p) {
            h.emitter = 100;
            h.emitter_target = Some(target);
        }
    }

    /// Trail particle draws (world random: 6 doubles and one integer per particle).
    fn heart_emit_particles(&mut self, p: [i32; 3], count: i32) {
        if self.heart_protector(p).is_none() {
            return;
        }
        self.heart_particle_draws(count);
    }

    fn heart_particle_draws(&mut self, count: i32) {
        let r = &mut self.mobs.rng;
        for _ in 0..count {
            for _ in 0..6 {
                let _ = r.next_double();
            }
            let _ = r.next_int_bound(40);
        }
    }

    /// Breadth-first traversal of pale oak logs (depth 2, 64 at most),
    /// shuffled directions; the first free face of a log receives a resin cluster.
    fn heart_spread_resin(&mut self, start: [i32; 3]) -> Option<[i32; 3]> {
        const DIRS: [([i32; 3], &str, &str); 6] = [
            ([0, -1, 0], "down", "up"),
            ([0, 1, 0], "up", "down"),
            ([0, 0, -1], "north", "south"),
            ([0, 0, 1], "south", "north"),
            ([-1, 0, 0], "west", "east"),
            ([1, 0, 0], "east", "west"),
        ];
        // Node marked visited when removed from
        // the queue, processed (a log tries its faces), counted, then neighbors added.
        let mut queue: std::collections::VecDeque<([i32; 3], i32)> = std::collections::VecDeque::new();
        let mut seen = hashbrown::HashSet::new();
        queue.push_back((start, 0));
        let mut count = 0;
        while let Some((pos, depth)) = queue.pop_front() {
            if !seen.insert(pos) {
                continue;
            }
            if self.btag_id(pos, crate::tag_bits::TagId::PaleOakLogs) {
                let order = self.shuffled_dirs();
                for di in order {
                    let (d, _, opposite) = DIRS[di];
                    let q = [pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]];
                    let mut st = self.st(q);
                    let name = self.state_name(st).to_owned();
                    if self.is_air(q) {
                        st = self.default_state("minecraft:resin_clump");
                    } else if name == "minecraft:water" && self.prop(q, "level").as_deref() == Some("0") {
                        let base = self.default_state("minecraft:resin_clump");
                        st = self.with_prop(base, "waterlogged", "true").unwrap_or(base);
                    }
                    if self.state_name(st) == "minecraft:resin_clump" {
                        let has = self.data.state_properties(st).any(|(k, v)| k == opposite && v == "true");
                        if !has && let Some(n) = self.with_prop(st, opposite, "true") {
                            self.set_update(q, n);
                            return Some(q);
                        }
                    }
                }
            }
            count += 1;
            if count >= 64 {
                return None;
            }
            if depth >= 2 {
                continue;
            }
            // Neighbors: logs only, shuffled order.
            let order = self.shuffled_dirs();
            for di in order {
                let d = DIRS[di].0;
                let q = [pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]];
                if self.btag_id(q, crate::tag_bits::TagId::PaleOakLogs) {
                    queue.push_back((q, depth + 1));
                }
            }
        }
        None
    }

    /// Shuffled copy of the directions with the world random (the game's Fisher-Yates).
    fn shuffled_dirs(&mut self) -> [usize; 6] {
        let mut v = [0usize, 1, 2, 3, 4, 5];
        for i in (1..6).rev() {
            let j = self.mobs.rng.next_int_bound(i as i32 + 1) as usize;
            v.swap(i, j);
        }
        v
    }

    /// Without a source, the creaking breaks apart; with a source (player who
    /// breaks the heart, explosion), it dies with its effects then tears down.
    pub(crate) fn heart_remove_protector(&mut self, p: [i32; 3], src: Option<(&str, Option<Ent>)>) {
        let Some(id) = self.heart_protector(p) else { return };
        if let Some(i) = self.mob_index(id)
            && let Some(mut c) = self.mobs.list[i].take()
        {
            match src {
                None => super::creaking::tear_down(self, &mut c),
                Some((ty, by)) => super::creaking::death_effects(self, &mut c, ty, by),
            }
            self.mobs.list[i] = Some(c);
        }
        if let Some(h) = self.mobs.rec.hearts.map.get_mut(&p) {
            h.link = None;
        }
        self.heart_store(p);
    }

    /// The player who breaks the heart kills its protector; experience (20 to
    /// 24) for a natural heart, except in creative.
    pub(crate) fn heart_player_will_destroy(&mut self, p: [i32; 3], state: u32) {
        if !is_heart_block(self.state_name(state)) || self.block_entity(p).is_none() {
            return;
        }
        self.heart_remove_protector(p, Some(("minecraft:player_attack", Some(Ent::Player))));
        self.heart_award_xp(p, state);
    }

    /// 20 to 24 points for a natural heart (neither creative nor spectator).
    fn heart_award_xp(&mut self, p: [i32; 3], state: u32) {
        let natural = self.data.state_properties(state).any(|(k, v)| k == "natural" && v == "true");
        if self.game_mode != 1 && self.game_mode != 3 && natural {
            let xp = self.mobs.rng.int_between(20, 24);
            if self.rule("minecraft:block_drops", true) {
                self.spawn_orbs([f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5], xp);
            }
        }
    }

    /// The reaction to a heart explosion (before the block's destruction): the protector dies with its
    /// effects, death attributed to the source; experience if the explosion comes from a player.
    pub(crate) fn heart_explosion_hit(&mut self, p: [i32; 3], state: u32, ty: &str, by: Option<Ent>) {
        if !is_heart_block(self.state_name(state)) || self.block_entity(p).is_none() {
            return;
        }
        self.heart_remove_protector(p, Some((ty, by)));
        if by == Some(Ent::Player) {
            self.heart_award_xp(p, state);
        }
    }

    /// Heart removed, protector broken apart.
    pub(crate) fn heart_pre_remove(&mut self, p: [i32; 3]) {
        if self.mobs.rec.hearts.map.contains_key(&p) {
            self.heart_remove_protector(p, None);
            self.mobs.rec.hearts.map.remove(&p);
        }
    }
}
