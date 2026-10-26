//! The world as seen by behaviours: reading states and fluids, placement with flags
//! (§ 1.8, course of a placement), single scheduler of notifications (neighbour changed, series
//! of 6, shape updates), destruction, dispatch to block families.

use hashbrown::{HashMap, HashSet};
use std::collections::VecDeque;
use std::sync::Arc;

use rc_client::Event;
use rc_data::{VersionData, flags};
use rc_world::block_index;

use super::attach::Sup;
use super::table::View;
use super::ticks::TickQueue;
use super::{Dir, JavaRandom, Kind, P, Table, below, flag, rel};
use crate::server::Server;

/// Scheduler job (§ 1.8).
#[derive(Debug, Clone, Copy)]
pub(crate) enum Job {
    Shape { dir: Dir, nstate: u32, pos: P, npos: P, flags: u32, limit: i32 },
    Simple { pos: P, block: u16 },
    Full { state: u32, pos: P, block: u16, moved: bool },
    Multi { src: P, block: u16, skip: Option<Dir>, idx: u8 },
}

/// Pending block event (§ 1.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct BlockEvent {
    pub pos: P,
    pub block: u16,
    pub a: i32,
    pub b: i32,
}

/// Moving block of a piston (block entity, § 3.9).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Mover {
    pub id: u64,
    pub moved: u32,
    pub dir: Dir,
    pub extending: bool,
    pub source: bool,
    pub progress: f32,
    pub progress_o: f32,
    pub last_ticked: i64,
}

impl Mover {
    /// Seen by physics (shape, push).
    pub fn physics(&self) -> rc_physics::piston::Moving {
        rc_physics::piston::Moving {
            moved: self.moved,
            dir: self.dir as u8,
            extending: self.extending,
            source: self.source,
            progress: self.progress,
        }
    }
}

/// Simulation state of the blocks of a world.
pub(crate) struct Sim {
    pub t: Arc<Table>,
    pub block_ticks: TickQueue,
    pub fluid_ticks: TickQueue,
    pub sub_tick: i64,
    pub events: VecDeque<BlockEvent>,
    pub event_set: HashSet<BlockEvent>,
    stack: Vec<Job>,
    added: Vec<Job>,
    count: i64,
    /// Rule `max-chained-neighbor-updates` (negative: unlimited).
    pub max_chained: i64,
    pub rng: JavaRandom,
    /// Generator of random tick positions (§ 1.5).
    pub lcg: i32,
    /// Recent torch toggles (§ 3.6), single list for the world.
    pub torches: VecDeque<(P, i64)>,
    /// False during a computation of dust power (§ 3.3).
    pub wire_signal: bool,
    /// Phase of the current world ticks (pistons, § 3.9).
    pub handling_tick: bool,
    pub movers: HashMap<P, Mover>,
    /// Block entities that tick (identifier, position), in order of addition (§ 1.7).
    pub tickers: Vec<(u64, P)>,
    pub pending_tickers: Vec<(u64, P)>,
    pub ticking_bes: bool,
    pub next_be: u64,
    /// Chunks whose saved ticks have been resumed.
    pub imported: HashSet<(i32, i32)>,
    pub entities: super::entities::Entities,
    /// Random-tick sections (chunk, section index): cache recomputed as needed.
    pub rt: HashMap<(i32, i32, usize), bool>,
    /// Hoppers: ticker identifier, transfer delay, last tick.
    pub hoppers: HashMap<P, (u64, i32, i64)>,
    /// Direction of the current piston push: moving blocks in that direction do not block
    /// the pushed entity (collision-free mechanism of the game).
    pub noclip: Option<u8>,
    /// Animated Shulker boxes: ticker identifier, state (0 closed, 1 opening,
    /// 2 open, 3 closing), lid progress.
    pub shulkers: HashMap<P, (u64, u8, f32)>,
    /// G6-F01: jukeboxes and campfires that tick: ticker identifier and kept state.
    pub be_ticks: HashMap<P, (u64, BeTick)>,
    /// G5-F10: brushings in progress of suspicious blocks (not saved, as in the game).
    pub brushing: HashMap<P, crate::brush::Brushing>,
}

/// Kept state of a ticking block entity (G6-F01).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BeTick {
    /// Jukebox with a disc: track in progress.
    Jukebox(Option<super::jukebox::Song>),
    /// Campfire: food present (cooking lit), cooking started (cooling
    /// off); with neither, the tick is skipped.
    Campfire { food: bool, warm: bool },
    /// G5-F10: End gateway (age held in the NBT `Age`).
    Gateway,
    /// G5-F11: beacon (column being examined, levels).
    Beacon(crate::beacon::BeaconTick),
    /// G6-F12: conduit (active, tick of the next short ambient sound).
    Conduit(super::conduit::ConduitTick),
}

impl Sim {
    pub fn new(data: &VersionData) -> Sim {
        // In test, seed follows the thread (like the other "clock" random draws, `rng::test_seed`).
        #[cfg(test)]
        let seed = crate::rng::test_seed();
        #[cfg(not(test))]
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        let mut rng = JavaRandom::new(seed);
        let lcg = rng.next_int();
        Sim {
            t: Arc::new(Table::new(data)),
            block_ticks: TickQueue::default(),
            fluid_ticks: TickQueue::default(),
            sub_tick: 0,
            events: VecDeque::new(),
            event_set: HashSet::new(),
            stack: Vec::new(),
            added: Vec::new(),
            count: 0,
            max_chained: 1_000_000,
            rng,
            lcg,
            torches: VecDeque::new(),
            wire_signal: true,
            handling_tick: false,
            movers: HashMap::new(),
            tickers: Vec::new(),
            pending_tickers: Vec::new(),
            ticking_bes: false,
            next_be: 1,
            imported: HashSet::new(),
            entities: Default::default(),
            rt: HashMap::new(),
            hoppers: HashMap::new(),
            noclip: None,
            shulkers: HashMap::new(),
            be_ticks: HashMap::new(),
            brushing: HashMap::new(),
        }
    }
}

/// Fluid state of a block state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fluid {
    /// 0 empty, 1 water, 2 lava.
    pub kind: u8,
    pub source: bool,
    pub amount: u8,
    pub falling: bool,
}

impl Fluid {
    pub const EMPTY: Fluid = Fluid { kind: 0, source: false, amount: 0, falling: false };

    pub fn is_empty(self) -> bool {
        self.kind == 0
    }

    /// Exact type of the game (source and flowing are two types): 0 empty, 1 water, 2 flowing
    /// water, 3 lava, 4 flowing lava.
    pub fn type_id(self) -> u8 {
        match (self.kind, self.source) {
            (0, _) => 0,
            (k, true) => k * 2 - 1,
            (k, false) => k * 2,
        }
    }

    pub fn own_height(self) -> f32 {
        f32::from(self.amount) / 9.0
    }
}

impl Server {
    #[inline]
    pub(crate) fn v(&self) -> View {
        View { t: self.bs.t.clone(), d: self.data.clone() }
    }

    pub(crate) fn tb(&self) -> Arc<Table> {
        self.bs.t.clone()
    }

    pub(crate) fn top_y(&self) -> i32 {
        self.dim.height.min_y + self.dim.height.sections as i32 * 16
    }

    pub(crate) fn in_build_height(&self, y: i32) -> bool {
        (self.dim.height.min_y..self.top_y()).contains(&y)
    }

    /// State at `p`; void air outside the height or chunk absent.
    #[inline]
    pub(crate) fn get(&self, p: P) -> u32 {
        crate::play::block_at(&self.chunks, self.dim.height.min_y, p)
            .unwrap_or_else(|| self.v().default_state(self.bs.t.id.void_air))
    }

    pub(crate) fn is_loaded(&self, p: P) -> bool {
        matches!(self.chunks.get(&(p[0] >> 4, p[2] >> 4)), Some(Some(_)))
    }

    /// The chunk of `p` and its 8 neighbours are loaded ("within block tick range", § 1.4).
    pub(crate) fn ticking_at(&self, p: P) -> bool {
        let (cx, cz) = (p[0] >> 4, p[2] >> 4);
        self.chunk_simulated_now((cx, cz))
            && (-1..=1).all(|dx| (-1..=1).all(|dz| matches!(self.chunks.get(&(cx + dx, cz + dz)), Some(Some(_)))))
    }

    /// Does a block entity at `p` tick this tick? Within the world border (column of the block)
    /// and in a chunk within block tick range. Same test for all block entities, whether they
    /// go through the common ticker list or through their own loop (furnaces, brewing stands,
    /// spawners): outside the border, a furnace no longer cooks, a hopper no longer transfers.
    pub(crate) fn be_ticking(&self, p: P) -> bool {
        Self::block_in_border(self.border_bounds(), p) && self.ticking_at(p)
    }

    pub(crate) fn fluid_of(&self, s: u32) -> Fluid {
        // Called several times per block in fluid ticks: neither a copy of the view (two
        // atomic counters) nor a name comparison, the type comes from the table.
        let info = self.data.states.get(s as usize).copied().unwrap_or_default();
        let kind = self.bs.t.fluid_kind.get(info.fluid as usize).copied().unwrap_or(0);
        if kind == 0 {
            return Fluid::EMPTY;
        }
        Fluid {
            kind,
            source: info.has(flags::FLUID_SOURCE),
            amount: info.fluid_amount,
            falling: info.has(flags::FLUID_FALLING),
        }
    }

    pub(crate) fn fluid_at(&self, p: P) -> Fluid {
        self.fluid_of(self.get(p))
    }

    /// Liquid block of a fluid state, air if empty.
    pub(crate) fn legacy_block(&self, f: Fluid) -> u32 {
        let t = &self.bs.t;
        let v = self.v();
        match f.kind {
            1 | 2 => {
                let b = if f.kind == 1 { t.id.water } else { t.id.lava };
                let level = if f.source { 0 } else { 8 - i32::from(f.amount.min(8)) + if f.falling { 8 } else { 0 } };
                v.set_int(v.default_state(b), "level", level)
            }
            _ => v.default_state(t.id.air),
        }
    }

    pub(crate) fn air(&self) -> u32 {
        self.states.air_state
    }

    /// Raw write into the section (no notification at all).
    pub(super) fn write_raw(&mut self, p: P, state: u32) -> Option<u32> {
        let min_y = self.dim.height.min_y;
        let ly = p[1] - min_y;
        let c = self.chunks.get_mut(&(p[0] >> 4, p[2] >> 4))?.as_mut()?;
        let si = usize::try_from(ly >> 4).ok().filter(|&i| i < c.sections.len())?;
        let idx = block_index((p[0] & 15) as usize, (ly & 15) as usize, (p[2] & 15) as usize);
        let old = c.sections[si].blocks.get(idx);
        if old == state {
            return None;
        }
        let air = &self.states;
        let direct = self.data.state_bits() as u8;
        c.sections[si].set_blocks(&[(idx as u16, state)], direct, |s| air.is_air(s));
        c.update_heightmaps(
            (p[0] & 15) as usize,
            p[1],
            (p[2] & 15) as usize,
            state,
            &air.heightmap_rules,
            self.dim.height,
        );
        c.changed[si] = true;
        c.dirty = true;
        let key = (p[0] >> 4, p[2] >> 4, si);
        if self.data.states.get(state as usize).is_some_and(|i| i.has(flags::RANDOM_TICK))
            || self.fluid_of(state).kind == 2
        {
            self.bs.rt.insert(key, true);
        } else if self.data.states.get(old as usize).is_some_and(|i| i.has(flags::RANDOM_TICK)) {
            self.bs.rt.remove(&key);
        }
        if self.light.matters(old, state) {
            let o = self.light_opts;
            // Consecutive duplicate dropped (`misc`): exact, nothing moved between the two, it
            // would join the same group without changing its box.
            if !(o.misc && self.relight.last() == Some(&p)) {
                self.relight.push(p);
            }
            if o.sky_list && self.light.matters_sky(old, state) && !(o.misc && self.relight_sky.last() == Some(&p)) {
                self.relight_sky.push(p);
            }
        }
        self.poi_block_changed(p, old, state); // F05: POI registry
        Some(old)
    }

    /// Placement with flags and shape depth 512.
    pub(crate) fn set_block(&mut self, p: P, state: u32, flags: u32) -> bool {
        self.set_block_limit(p, state, flags, 512)
    }

    /// Placement (§ 1.8, course in 11 steps).
    pub(crate) fn set_block_limit(&mut self, p: P, state: u32, flags: u32, limit: i32) -> bool {
        if !self.in_build_height(p[1]) {
            return false;
        }
        let Some(old) = self.write_raw(p, state) else { return false };
        let v = self.v();
        let (ob, nb) = (v.block(old), v.block(state));
        let block_changed = ob != nb;
        let moved = flags & flag::MOVED != 0;
        if block_changed && v.has_block_entity(old) && !self.keeps_block_entity(ob, nb) {
            // F14: creaking heart, shrieker.
            if flags & flag::SKIP_BE_SIDE_EFFECTS == 0 {
                self.rec_pre_remove(p, old);
                // G6-F01: disc, book, pot item given back.
                match self.v().kind(old) {
                    Kind::Jukebox => self.jukebox_pre_remove(p, old),
                    Kind::Lectern => self.lectern_pre_remove(p, old),
                    Kind::DecoratedPot => self.pot_pre_remove(p),
                    _ => {}
                }
            } else if self.v().kind(old) == Kind::Jukebox {
                // The removal of the entity happens even without removal effects.
                self.bs.be_ticks.remove(&p);
                self.game_event_block("minecraft:jukebox_stop_play", p, None, Some(old));
                self.level_event(1011, p, 0);
            }
            // G5-F11: the removed beacon turns off (sound), with or without removal effects.
            if self.v().kind(old) == Kind::Beacon {
                self.beacon_removed(p);
            }
            self.remove_block_entity(p);
        }
        let new_kind = self.v().kind(state);
        if (block_changed || matches!(new_kind, Kind::Rail | Kind::PoweredRail | Kind::DetectorRail))
            && (flags & flag::NEIGHBORS != 0 || moved)
        {
            self.affect_after_removal(old, p, moved);
        }
        if self.v().block(self.get(p)) != nb {
            return false;
        }
        if flags & flag::SKIP_ON_PLACE == 0 {
            self.on_place(state, p, old, moved);
        }
        if self.v().has_block_entity(state) && self.v().block(self.get(p)) == nb {
            self.ensure_block_entity(p, state);
            // G6-F01: jukebox ticker according to `has_record`.
            if new_kind == Kind::Jukebox && self.get(p) == state {
                self.jukebox_ticker_sync(p, state);
            }
        }
        if self.get(p) == state {
            if flags & flag::CLIENTS != 0 {
                self.changed.push(p);
            }
            if flags & flag::NEIGHBORS != 0 {
                self.update_neighbors_at(p, ob);
                if self.v().has_analog(state) {
                    self.update_neighbour_for_output_signal(p, nb);
                }
            }
            if flags & flag::KNOWN_SHAPE == 0 && limit > 0 {
                let nf = flags & !(flag::NEIGHBORS | flag::NO_DROPS);
                self.update_indirect_shapes(old, p, nf, limit - 1);
                self.update_neighbour_shapes(state, p, nf, limit - 1);
                self.update_indirect_shapes(state, p, nf, limit - 1);
            }
        }
        true
    }

    /// The new block keeps the block entity of the old one:
    /// copper chests among themselves, copper golem statues among themselves (oxidation, wax,
    /// unwaxing); the contents or the pose are not lost.
    fn keeps_block_entity(&self, ob: u16, nb: u16) -> bool {
        ["minecraft:copper_chests", "minecraft:copper_golem_statues"]
            .iter()
            .any(|t| self.tag(t, nb) && self.tag(t, ob))
    }

    /// Flags 3.
    pub(crate) fn set_update(&mut self, p: P, state: u32) -> bool {
        self.set_block(p, state, flag::ALL)
    }

    /// Removal (remaining fluid): flags 3 (+64 by piston).
    pub(crate) fn remove_block(&mut self, p: P, moved: bool) -> bool {
        let f = self.fluid_at(p);
        let s = if f.is_empty() { self.air() } else { self.legacy_block(Fluid { falling: false, ..f }) };
        self.set_block(p, s, flag::ALL | if moved { flag::MOVED } else { 0 })
    }

    /// Destruction (loot not handled): particles and sound (event 2001), remaining fluid.
    pub(crate) fn destroy_block(&mut self, p: P, drop: bool, limit: i32) -> bool {
        self.destroy_block_by(p, drop, limit, None)
    }

    /// Destruction with the entity that breaks: if the placement succeeds, `block_destroy` event
    /// (entity, old state).
    pub(crate) fn destroy_block_by(
        &mut self,
        p: P,
        drop: bool,
        limit: i32,
        by: Option<crate::mobs::recent::vibration::Src>,
    ) -> bool {
        let state = self.get(p);
        if self.v().is_air(state) {
            return false;
        }
        // Break particles; the straw bed has its own, without sound (2014).
        let straw = self.data.block_of(state).is_some_and(|b| b.name == "minecraft:straw_bed");
        self.level_event(if straw { 2014 } else { 2001 }, p, state as i32);
        // Loot before removal (empty tool), then after-break effects.
        if drop {
            self.drop_resources(p, state, None);
        }
        let f = self.fluid_of(state);
        let s = if f.is_empty() { self.air() } else { self.legacy_block(Fluid { falling: false, ..f }) };
        let done = self.set_block_limit(p, s, flag::ALL, limit);
        if done {
            let c = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
            self.game_event_src("minecraft:block_destroy", c, by, Some(state));
        }
        done
    }

    /// Adventure mode: the item carries a `can_break` of which a predicate accepts the block (name or
    /// `#...` tag; state and data restrictions are not evaluated).
    pub(crate) fn adventure_can_break(&self, item: Option<&crate::player::Stack>, state: u32) -> bool {
        self.adventure_allows(item, state, "minecraft:can_break")
    }

    /// Adventure mode: like `adventure_can_break`, for the `can_place_on` component (use of an
    /// item on the clicked block).
    pub(crate) fn adventure_can_place_on(&self, item: Option<&crate::player::Stack>, state: u32) -> bool {
        self.adventure_allows(item, state, "minecraft:can_place_on")
    }

    fn adventure_allows(&self, item: Option<&crate::player::Stack>, state: u32, component: &str) -> bool {
        use rc_proto::nbt::Nbt;
        let Some(c) = item.and_then(|s| s.components.as_ref()).and_then(|c| c.get(component)) else {
            return false;
        };
        let preds: Vec<&Nbt> = match c {
            Nbt::List(l) => l.iter().collect(),
            Nbt::Compound(_) => match c.get("predicates") {
                Some(Nbt::List(l)) => l.iter().collect(),
                _ => vec![c],
            },
            _ => return false,
        };
        let name = self.state_name(state);
        let id = u32::from(self.v().block(state));
        let hit = |k: &str| match k.strip_prefix('#') {
            Some(tag) => self
                .block_tags
                .get(tag)
                .or_else(|| rc_data::ids::with_key(&["minecraft:", tag], |k| self.block_tags.get(k)))
                .is_some_and(|t| t.binary_search(&id).is_ok()),
            None => k == name,
        };
        preds.iter().any(|pr| match pr.get("blocks") {
            None => true,
            Some(Nbt::String(k)) => hit(k),
            Some(Nbt::List(l)) => l.iter().any(|e| e.as_str().is_some_and(hit)),
            _ => false,
        })
    }

    /// Priming of a TNT block: refused without the rule `tnt_explodes`;
    /// otherwise primed TNT, priming sound at its position, `prime_fuse` event of the author.
    pub(crate) fn tnt_prime_block(&mut self, p: P, by: Option<crate::mobs::mob::Ent>) -> bool {
        self.tnt_prime_block_with(p, by, None)
    }

    /// Like `tnt_prime_block`, with the lighting item: an adventure-mode player only primes
    /// if this item has `can_break` for the block. The primed TNT keeps its author.
    pub(crate) fn tnt_prime_block_with(
        &mut self,
        p: P,
        by: Option<crate::mobs::mob::Ent>,
        item: Option<&crate::player::Stack>,
    ) -> bool {
        if !self.rule("minecraft:tnt_explodes", true) {
            return false;
        }
        if by == Some(crate::mobs::mob::Ent::Player) && self.game_mode == 2 {
            let state = self.get(p);
            if !self.adventure_can_break(item, state) {
                return false;
            }
        }
        self.tnt_prime_by(p, by);
        let at = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
        self.world_sound("minecraft:entity.tnt.primed", 4, at, 1.0, 1.0);
        self.game_event_block("minecraft:prime_fuse", p, by, None);
        true
    }

    /// Loot of a block broken by something other than a player: table `blocks/<name>` (tool given or empty, block
    /// entity still in place), items placed at the center of the block, then the after-break effects
    /// (experience of ores without Silk Touch, silverfish of infested blocks).
    pub(crate) fn drop_resources(&mut self, p: P, state: u32, tool: Option<&crate::player::Stack>) {
        let drops = self.block_drops(p, state, tool);
        for d in drops {
            self.pop_resource(p, d);
        }
        let Some(block) = self.data.block_of(state).map(|b| b.name.clone()) else { return };
        self.drop_block_xp(p, &block, tool);
        self.infested_broken(p, &block, tool);
    }

    /// Items drawn from the loot table of the block (tool present but possibly empty).
    pub(crate) fn block_drops(
        &mut self,
        p: P,
        state: u32,
        tool: Option<&crate::player::Stack>,
    ) -> Vec<crate::player::Stack> {
        if !self.rule("minecraft:block_drops", true) {
            return Vec::new();
        }
        let Some(block) = self.data.block_of(state).map(|b| b.name.clone()) else { return Vec::new() };
        let props: Vec<(String, String)> =
            self.data.state_properties(state).map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
        let be = self.block_entity(p).cloned();
        let table_id = format!("minecraft:blocks/{}", block.trim_start_matches("minecraft:"));
        let tags = &self.block_tags;
        let data = &self.data;
        let tag_fn = |tag: &str, name: &str| {
            let id = data.blocks.iter().position(|b| b.name == name).map(|i| i as u32);
            id.is_some_and(|id| tags.get(tag).is_some_and(|v| v.binary_search(&id).is_ok()))
        };
        let seq =
            self.life.pack.get("loot_table", &table_id).and_then(|t| t["random_sequence"].as_str().map(str::to_owned));
        // W8c: generator taken out for the time of the draw (the world is lent to the conditions).
        let mut rng = match &seq {
            Some(s) => *self.life.seqs.get(s),
            None => self.life.rng,
        };
        let drops = {
            let world = self.loot_world(crate::progress::pred::Ctx {
                origin: Some([f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5]),
                block: Some(state),
                tool: Some(tool),
                ..Default::default()
            });
            let loot = crate::loot::Loot { pack: &self.life.pack, items: &self.life.items, block_tag: &tag_fn };
            let ctx = crate::loot::Ctx {
                block: Some((block.as_str(), props)),
                tool: Some(tool),
                block_entity: be.as_ref(),
                world: Some(&world),
                ..Default::default()
            };
            loot.drops(&table_id, &ctx, &mut rng)
        };
        match &seq {
            Some(s) => *self.life.seqs.get(s) = rng,
            None => self.life.rng = rng,
        }
        drops
    }

    pub(crate) fn level_event(&self, kind: i32, p: P, data: i32) {
        let _ = self.send(Event::LevelEvent { kind, pos: p, data, global: false });
    }

    /// World event "heard everywhere" (wither spawning, dragon death, End
    /// portal): rule `global_sound_events` true, sent to each player, position brought back
    /// to 32 blocks from them; false, ordinary event.
    pub(crate) fn global_level_event(&self, kind: i32, p: P, data: i32) {
        if !self.rule("minecraft:global_sound_events", true) {
            self.level_event(kind, p, data);
            return;
        }
        let c = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        self.send_each_player_at(|at| {
            let d = [c[0] - at[0], c[1] - at[1], c[2] - at[2]];
            let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            let sound = if d2 < 32.0 * 32.0 {
                c
            } else {
                let l = d2.sqrt();
                let n = if l < f64::from(1.0e-5f32) { [0.0; 3] } else { d.map(|v| v / l) };
                [at[0] + n[0] * 32.0, at[1] + n[1] * 32.0, at[2] + n[2] * 32.0]
            };
            Event::LevelEvent { kind, pos: crate::mobs::mth::block_of(sound), data, global: true }
        });
    }

    /// The update of a block or its destruction.
    pub(crate) fn update_or_destroy(&mut self, old: u32, new: u32, p: P, flags: u32, limit: i32) {
        if new != old {
            if self.v().is_air(new) {
                self.destroy_block(p, flags & flag::NO_DROPS == 0, limit);
            } else {
                self.set_block_limit(p, new, flags & !flag::NO_DROPS, limit);
            }
        }
    }

    /// State adjusted by the shapes of the 6 neighbours.
    pub(crate) fn update_from_neighbour_shapes(&mut self, state: u32, p: P) -> u32 {
        let mut s = state;
        for d in Dir::SHAPE_ORDER {
            let np = rel(p, d);
            let ns = self.get(np);
            s = self.update_shape(s, p, d, np, ns);
        }
        s
    }

    // --- Notification scheduler (§ 1.8). ---

    fn add_and_run(&mut self, job: Job) {
        let running = self.bs.count > 0;
        let max = self.bs.max_chained;
        let too_many = max >= 0 && self.bs.count >= max;
        self.bs.count += 1;
        if !too_many {
            if running {
                self.bs.added.push(job);
            } else {
                self.bs.stack.push(job);
            }
        } else if self.bs.count - 1 == max {
            eprintln!("singleplayer: too many chained neighbour updates, the rest is ignored");
        }
        if !running {
            self.run_updates();
        }
    }

    fn run_updates(&mut self) {
        while !self.bs.stack.is_empty() || !self.bs.added.is_empty() {
            while let Some(j) = self.bs.added.pop() {
                self.bs.stack.push(j);
            }
            // `added` emptied in reverse order: the first added is at the top.
            let top = self.bs.stack.len() - 1;
            loop {
                let mut job = self.bs.stack[top];
                let more = self.run_step(&mut job);
                if !more {
                    self.bs.stack.pop();
                    break;
                }
                self.bs.stack[top] = job;
                if !self.bs.added.is_empty() {
                    break;
                }
            }
        }
        self.bs.stack.clear();
        self.bs.added.clear();
        self.bs.count = 0;
    }

    fn run_step(&mut self, job: &mut Job) -> bool {
        match *job {
            Job::Shape { dir, nstate, pos, npos, flags, limit } => {
                self.execute_shape_update(dir, pos, npos, nstate, flags, limit);
                false
            }
            Job::Simple { pos, block } => {
                let s = self.get(pos);
                self.neighbor_changed(s, pos, block, false);
                false
            }
            Job::Full { state, pos, block, moved } => {
                self.neighbor_changed(state, pos, block, moved);
                false
            }
            Job::Multi { src, block, skip, ref mut idx } => {
                let d = Dir::UPDATE_ORDER[*idx as usize];
                *idx += 1;
                let np = rel(src, d);
                let s = self.get(np);
                self.neighbor_changed(s, np, block, false);
                if (*idx as usize) < 6 && Some(Dir::UPDATE_ORDER[*idx as usize]) == skip {
                    *idx += 1;
                }
                (*idx as usize) < 6
            }
        }
    }

    fn execute_shape_update(&mut self, dir: Dir, pos: P, npos: P, nstate: u32, flags: u32, limit: i32) {
        let cur = self.get(pos);
        if flags & flag::SKIP_WIRE_SHAPE != 0 && self.v().kind(cur) == Kind::Wire {
            return;
        }
        let new = self.update_shape(cur, pos, dir, npos, nstate);
        self.update_or_destroy(cur, new, pos, flags, limit);
    }

    /// "Neighbour changed" to the 6 neighbours of `p` (order west, east, down, up, north, south).
    pub(crate) fn update_neighbors_at(&mut self, p: P, block: u16) {
        self.add_and_run(Job::Multi { src: p, block, skip: None, idx: 0 });
    }

    pub(crate) fn update_neighbors_except(&mut self, p: P, block: u16, skip: Dir) {
        let idx = u8::from(Dir::UPDATE_ORDER[0] == skip);
        self.add_and_run(Job::Multi { src: p, block, skip: Some(skip), idx });
    }

    /// Simple "neighbour changed" at `p` (state re-read at the time of the step).
    pub(crate) fn neighbor_changed_at(&mut self, p: P, block: u16) {
        self.add_and_run(Job::Simple { pos: p, block });
    }

    /// Full "neighbour changed" (state provided).
    pub(crate) fn neighbor_changed_full(&mut self, state: u32, p: P, block: u16, moved: bool) {
        self.add_and_run(Job::Full { state, pos: p, block, moved });
    }

    /// Shape update sent to `pos`: its neighbour `npos` (in direction `dir` seen from
    /// `pos`) became `nstate`.
    pub(crate) fn neighbor_shape_changed(&mut self, dir: Dir, pos: P, npos: P, nstate: u32, flags: u32, limit: i32) {
        self.add_and_run(Job::Shape { dir, nstate, pos, npos, flags, limit });
    }

    /// Shapes of the 6 neighbours (order west, east, north, south, down, up).
    pub(crate) fn update_neighbour_shapes(&mut self, state: u32, p: P, flags: u32, limit: i32) {
        for d in Dir::SHAPE_ORDER {
            let np = rel(p, d);
            self.neighbor_shape_changed(d.opposite(), np, p, state, flags, limit);
        }
    }

    /// "Indirect" shapes (only dust has them, § 3.3).
    pub(crate) fn update_indirect_shapes(&mut self, state: u32, p: P, flags: u32, limit: i32) {
        if self.v().kind(state) == Kind::Wire {
            self.wire_indirect_shapes(state, p, flags, limit);
        }
    }

    /// Comparator update (§ 3.5).
    pub(crate) fn update_neighbour_for_output_signal(&mut self, p: P, block: u16) {
        for d in Dir::HORIZONTAL {
            let mut q = rel(p, d);
            if !self.is_loaded(q) {
                continue;
            }
            let s = self.get(q);
            if self.v().kind(s) == Kind::Comparator {
                self.neighbor_changed_full(s, q, block, false);
            } else if self.v().conductor(s) {
                q = rel(q, d);
                let s = self.get(q);
                if self.v().kind(s) == Kind::Comparator {
                    self.neighbor_changed_full(s, q, block, false);
                }
            }
        }
    }

    // --- Block entities (data kept in the chunk NBT). ---

    pub(crate) fn be_index(&self, p: P) -> Option<usize> {
        let c = self.chunks.get(&(p[0] >> 4, p[2] >> 4))?.as_ref()?;
        c.block_entities.iter().position(|v| {
            crate::nbtio::int(v, "x") == Some(i64::from(p[0]))
                && crate::nbtio::int(v, "y") == Some(i64::from(p[1]))
                && crate::nbtio::int(v, "z") == Some(i64::from(p[2]))
        })
    }

    pub(crate) fn be_get(&self, p: P) -> Option<&rc_proto::nbt::Nbt> {
        let i = self.be_index(p)?;
        self.chunks.get(&(p[0] >> 4, p[2] >> 4))?.as_ref()?.block_entities.get(i)
    }

    pub(crate) fn be_get_mut(&mut self, p: P) -> Option<&mut rc_proto::nbt::Nbt> {
        let i = self.be_index(p)?;
        let c = self.chunks.get_mut(&(p[0] >> 4, p[2] >> 4))?.as_mut()?;
        c.dirty = true;
        c.block_entities.get_mut(i)
    }

    pub(crate) fn remove_block_entity(&mut self, p: P) {
        self.bs.movers.remove(&p);
        if let Some(i) = self.be_index(p)
            && let Some(Some(c)) = self.chunks.get_mut(&(p[0] >> 4, p[2] >> 4))
        {
            c.block_entities.remove(i);
            c.dirty = true;
        }
    }

    /// Creates the block entity of a new state if missing (minimal NBT).
    fn ensure_block_entity(&mut self, p: P, state: u32) {
        if self.be_index(p).is_some() || self.v().kind(state) == Kind::MovingPiston {
            return;
        }
        let name = self.data.block_of(state).map(|b| b.name.clone()).unwrap_or_default();
        let id = crate::blocks::entities::block_entity_id(&name);
        let Some(id) = id else { return };
        use rc_proto::nbt::Nbt;
        let mut fields = vec![
            ("id".to_owned(), Nbt::String(id.to_owned())),
            ("x".to_owned(), Nbt::Int(p[0])),
            ("y".to_owned(), Nbt::Int(p[1])),
            ("z".to_owned(), Nbt::Int(p[2])),
        ];
        if id == "minecraft:comparator" {
            fields.push(("OutputSignal".to_owned(), Nbt::Int(0)));
        }
        if let Some(Some(c)) = self.chunks.get_mut(&(p[0] >> 4, p[2] >> 4)) {
            c.block_entities.push(Nbt::Compound(fields));
            c.dirty = true;
        }
        self.after_block_entity_created(p, state);
    }

    pub(crate) fn be_int(&self, p: P, key: &str) -> i32 {
        self.be_get(p).and_then(|v| crate::nbtio::int(v, key)).unwrap_or(0) as i32
    }

    pub(crate) fn be_set_int(&mut self, p: P, key: &str, value: i32) {
        if let Some(v) = self.be_get_mut(p) {
            crate::nbtio::set(v, key, rc_proto::nbt::Nbt::Int(value));
        }
    }

    // --- Scheduled ticks. ---

    pub(crate) fn schedule_tick(&mut self, p: P, block: u16, delay: i32) {
        self.schedule_tick_priority(p, block, delay, 0);
    }

    pub(crate) fn schedule_tick_priority(&mut self, p: P, block: u16, delay: i32, priority: i8) {
        let sub = self.bs.sub_tick;
        self.bs.sub_tick += 1;
        if !self.is_loaded(p) {
            return;
        }
        let at = self.time + i64::from(delay);
        if self.bs.block_ticks.schedule(p, u32::from(block), at, priority, sub) {
            self.mark_dirty(p);
        }
    }

    pub(crate) fn schedule_fluid_tick(&mut self, p: P, fluid_type: u8, delay: i32) {
        let sub = self.bs.sub_tick;
        self.bs.sub_tick += 1;
        if !self.is_loaded(p) {
            return;
        }
        let at = self.time + i64::from(delay);
        if self.bs.fluid_ticks.schedule(p, u32::from(fluid_type), at, 0, sub) {
            self.mark_dirty(p);
        }
    }

    pub(crate) fn has_scheduled_tick(&self, p: P, block: u16) -> bool {
        self.bs.block_ticks.has(p, u32::from(block))
    }

    pub(crate) fn will_tick_this_tick(&self, p: P, block: u16) -> bool {
        self.bs.block_ticks.will_tick_this_tick(p, u32::from(block))
    }

    fn mark_dirty(&mut self, p: P) {
        if let Some(Some(c)) = self.chunks.get_mut(&(p[0] >> 4, p[2] >> 4)) {
            c.dirty = true;
        }
    }

    pub(crate) fn block_event(&mut self, p: P, block: u16, a: i32, b: i32) {
        let e = BlockEvent { pos: p, block, a, b };
        if self.bs.event_set.insert(e) {
            self.bs.events.push_back(e);
        }
    }

    // --- Dispatch. ---

    pub(crate) fn on_place(&mut self, state: u32, p: P, old: u32, moved: bool) {
        let v = self.v();
        let same_block = v.block(old) == v.block(state);
        if !same_block && self.state_name(state) == "minecraft:sponge" {
            self.sponge_absorb(p);
            return;
        }
        match v.kind(state) {
            Kind::Wire => {
                if !same_block {
                    self.wire_on_place(state, p);
                }
            }
            Kind::Repeater | Kind::Comparator => self.diode_update_in_front(p, state),
            Kind::RedstoneTorch | Kind::RedstoneWallTorch => self.torch_notify(p, state),
            Kind::Observer => {
                if !same_block {
                    self.observer_on_place(state, p);
                }
            }
            Kind::Piston { .. } => {
                if !same_block && !self.bs.movers.contains_key(&p) && self.be_get(p).is_none() {
                    self.piston_check(p, state);
                }
            }
            Kind::Water | Kind::Lava => self.liquid_on_place(state, p),
            Kind::Falling { .. } | Kind::Anvil | Kind::ConcretePowder => {
                let b = v.block(state);
                self.schedule_tick(p, b, 2);
            }
            Kind::DragonEgg => {
                let b = v.block(state);
                self.schedule_tick(p, b, 5);
            }
            Kind::Fire | Kind::SoulFire => self.fire_on_place(state, p, old),
            Kind::Tnt => {
                if !same_block && self.has_neighbor_signal(p) && self.tnt_prime_block(p, None) {
                    self.remove_block(p, false);
                }
            }
            Kind::Hopper => {
                if !same_block {
                    self.hopper_check_powered(p, state);
                }
            }
            Kind::CopperBulb => {
                if !same_block {
                    self.copper_bulb_check(p, state);
                }
            }
            Kind::Rail | Kind::PoweredRail | Kind::DetectorRail if !same_block => {
                self.rail_on_place(p, state, moved);
            }
            Kind::Tripwire if !same_block => self.tripwire_update_source(p, state),
            Kind::Scaffolding => {
                let b = v.block(state);
                self.schedule_tick(p, b, 1);
            }
            Kind::Support(Sup::CoralBelow { alive: true } | Sup::CoralWall { alive: true }) => {
                self.coral_try_schedule_death(state, p);
            }
            Kind::Shelf => self.shelf_on_place(p, state, old), // G6-F01: side chain
            // Frosted ice: melt scheduled at each placement (age included), 60 to 120 ticks; the
            // shorter delay drawn afterwards by its tick is then ignored (26.3).
            Kind::FrostedIce => {
                let delay = 60 + self.bs.rng.next_int_bound(61);
                let b = v.block(state);
                self.schedule_tick(p, b, delay);
            }
            // Lit target placed without a scheduled tick (command, structure): turned off (flags 18).
            Kind::Target if !same_block => {
                let b = v.block(state);
                if v.get_int(state, "power") > 0 && !self.has_scheduled_tick(p, b) {
                    let off = v.set_int(state, "power", 0);
                    self.set_block(p, off, flag::CLIENTS | flag::KNOWN_SHAPE);
                }
            }
            _ => match self.state_name(state) {
                // Composter placed at level 7: goes to 8 in 20 ticks.
                "minecraft:composter" if v.get_int(state, "level") == 7 => {
                    let b = v.block(state);
                    self.schedule_tick(p, b, 20);
                }
                // Wet sponge where water evaporates: dried at once (smoke 2009, sound).
                "minecraft:wet_sponge" if self.dim.name == "minecraft:the_nether" => {
                    let dry = self.default_state("minecraft:sponge");
                    self.set_update(p, dry);
                    self.level_event(2009, p, 0);
                    let pitch = (1.0 + self.bs.rng.next_float() * 0.2) * 0.7;
                    self.block_sound(p, "minecraft:block.wet_sponge.dries", 1.0, pitch);
                }
                _ => {}
            },
        }
        // G6-F10: sculk sensor placed lit without a scheduled tick → turned off.
        self.sensor_on_place(state, p, same_block);
        // F06: carved pumpkin or lantern placed → golem construction.
        self.golem_on_place(state, p, old);
        // Lightning rod placed already powered (copper stripped by lightning): scheduled turn-off.
        self.lightning_rod_on_place(state, p, old);
        // F10: frogspawn, sniffer egg (scheduled hatching).
        self.bz_egg_on_place(state, p);
    }

    pub(crate) fn affect_after_removal(&mut self, old: u32, p: P, moved: bool) {
        let v = self.v();
        match v.kind(old) {
            Kind::Wire => {
                if !moved {
                    self.wire_on_remove(old, p);
                }
            }
            Kind::Repeater | Kind::Comparator => {
                if !moved {
                    self.diode_update_in_front(p, old);
                }
            }
            Kind::RedstoneTorch | Kind::RedstoneWallTorch => {
                if !moved {
                    self.torch_notify(p, old);
                }
            }
            Kind::Observer => self.observer_on_remove(old, p),
            Kind::Lever | Kind::Button { .. } => {
                if !moved && v.get_bool(old, "powered") {
                    self.attached_update_neighbours(p, old);
                }
            }
            Kind::Plate { .. } | Kind::WeightedPlate { .. } => {
                if !moved && self.plate_signal(old) > 0 {
                    self.plate_update_neighbours(p, old);
                }
            }
            Kind::PistonHead => self.piston_head_removed(old, p),
            Kind::Rail | Kind::PoweredRail | Kind::DetectorRail => self.rail_after_removal(old, p, moved),
            Kind::Chest | Kind::Hopper | Kind::Dispenser | Kind::Dropper => {
                self.update_neighbour_for_output_signal(p, v.block(old));
            }
            Kind::DaylightDetector | Kind::Target => {}
            // G6-F01: shelf (comparators, detached neighbours); powered lectern (below).
            Kind::Shelf => {
                self.update_neighbour_for_output_signal(p, v.block(old));
                self.shelf_powering_down(p, old);
            }
            Kind::Lectern => {
                if v.get_bool(old, "powered") {
                    self.update_neighbors_at(below(p), v.block(old));
                }
            }
            Kind::TripwireHook if !moved => self.hook_on_removed(old, p),
            Kind::Tripwire if !moved => {
                let s = v.set_bool(old, "powered", true);
                self.tripwire_update_source(p, s);
            }
            _ if self.sensor_after_removal(old, p) => {} // G6-F10: active sensor removed
            _ if self.lightning_rod_removed(old, p) => {} // powered lightning rod removed
            _ => {
                if v.has_analog(old) && v.has_block_entity(old) {
                    self.update_neighbour_for_output_signal(p, v.block(old));
                }
            }
        }
    }

    pub(crate) fn neighbor_changed(&mut self, state: u32, p: P, from: u16, moved: bool) {
        if self.state_name(state) == "minecraft:sponge" {
            self.sponge_absorb(p);
            return;
        }
        let v = self.v();
        match v.kind(state) {
            Kind::Wire => self.wire_neighbor_changed(state, p),
            Kind::Repeater | Kind::Comparator => self.diode_neighbor_changed(state, p),
            Kind::RedstoneTorch | Kind::RedstoneWallTorch => self.torch_neighbor_changed(state, p),
            Kind::Lamp => self.lamp_neighbor_changed(state, p),
            Kind::Piston { .. } => self.piston_check(p, state),
            Kind::PistonHead => self.piston_head_neighbor_changed(state, p, from),
            Kind::Door { .. } => self.door_neighbor_changed(state, p, from),
            Kind::Trapdoor { .. } => self.trapdoor_neighbor_changed(state, p),
            Kind::FenceGate => self.gate_neighbor_changed(state, p),
            Kind::Water | Kind::Lava => self.liquid_neighbor_changed(state, p),
            Kind::Dispenser | Kind::Dropper | Kind::Crafter => self.dispenser_neighbor_changed(state, p),
            Kind::Hopper => self.hopper_check_powered(p, state),
            Kind::Tnt => {
                if self.has_neighbor_signal(p) && self.tnt_prime_block(p, None) {
                    self.remove_block(p, false);
                }
            }
            Kind::NoteBlock => self.note_neighbor_changed(state, p),
            Kind::CopperBulb => self.copper_bulb_check(p, state),
            Kind::Rail | Kind::PoweredRail | Kind::DetectorRail => self.rail_neighbor_changed(state, p, from),
            // Frosted ice warned by a neighbouring frosted ice: melts if fewer than two
            // frosted neighbours remain.
            Kind::FrostedIce if from == v.block(state) => {
                let b = v.block(state);
                let near = Dir::ALL.iter().filter(|&&d| self.v().block(self.get(rel(p, d))) == b).count();
                if near < 2 {
                    self.frosted_ice_melt(p);
                }
            }
            Kind::Shelf => self.shelf_neighbor_changed(p, state), // G6-F01
            // Bell: rings on the rising edge, powered state tracked (flags 3).
            Kind::Bell => {
                let signal = self.has_neighbor_signal(p);
                if signal != v.get_bool(state, "powered") {
                    if signal {
                        self.bell_attempt_ring(p, None, None);
                    }
                    let ns = self.v().set_bool(state, "powered", signal);
                    self.set_update(p, ns);
                }
            }
            // Powered head: the animation (dragon mouth, piglin) follows the signal (flag 2).
            Kind::Skull => {
                let signal = self.has_neighbor_signal(p);
                if v.has(state, "powered") && signal != v.get_bool(state, "powered") {
                    let ns = self.v().set_bool(state, "powered", signal);
                    self.set_block(p, ns, flag::CLIENTS);
                }
            }
            Kind::BigDripleaf => self.dripleaf_neighbor_changed(state, p),
            _ => self.cmdblock_neighbor_changed(state, p), // S5
        }
        let _ = moved;
    }

    pub(crate) fn tick_block(&mut self, state: u32, p: P) {
        let v = self.v();
        match v.kind(state) {
            Kind::Repeater => self.diode_tick(state, p),
            Kind::Comparator => self.comparator_refresh(p, state),
            Kind::RedstoneTorch | Kind::RedstoneWallTorch => self.torch_tick(state, p),
            Kind::Lamp => self.lamp_tick(state, p),
            Kind::Observer => self.observer_tick(state, p),
            Kind::Button { .. } => self.button_tick(state, p),
            Kind::Plate { .. } | Kind::WeightedPlate { .. } => self.plate_tick(state, p),
            Kind::DetectorRail => self.detector_rail_tick(state, p), // G5-F06
            Kind::Dispenser | Kind::Dropper | Kind::Crafter => self.dispenser_tick(state, p),
            Kind::Target => self.target_tick(state, p),
            Kind::Falling { .. } | Kind::Anvil | Kind::ConcretePowder | Kind::DragonEgg => self.falling_tick(state, p),
            Kind::TripwireHook => self.hook_tick(state, p),
            Kind::Tripwire => self.tripwire_tick(state, p),
            Kind::Bamboo { sapling: false } => self.bamboo_tick(p),
            Kind::Scaffolding => self.scaffold_tick(state, p),
            Kind::HangingMoss => self.hanging_moss_tick(state, p),
            Kind::Lectern => self.lectern_tick(state, p), // G6-F01: end of the page pulse
            Kind::GrowingHead { up } | Kind::GrowingBody { up } => self.growing_tick(state, p, up),
            Kind::BigDripleaf => self.dripleaf_tick(state, p),
            // Stem, chorus: broken (loot) if they no longer hold.
            Kind::BigDripleafStem | Kind::ChorusFlower | Kind::ChorusPlant => {
                if !self.can_survive(state, p) {
                    self.destroy_block(p, true, 512);
                }
            }
            Kind::Support(sup @ (Sup::CoralBelow { .. } | Sup::CoralWall { .. } | Sup::CoralBlock)) => {
                self.coral_tick(sup, state, p);
            }
            _ if self.bz_egg_tick(state, p) => {}    // F10: frogspawn, sniffer egg
            _ if self.cmdblock_tick(state, p) => {}  // S5: command blocks
            _ if self.rec_block_tick(state, p) => {} // F14: heart, shrieker, dried ghast
            _ if self.lightning_rod_tick(state, p) => {} // struck lightning rod that turns off
            _ => self.nature_tick(state, p),
        }
    }

    pub(crate) fn random_tick(&mut self, state: u32, p: P) {
        if self.pig_portal_random_tick(state, p) {
            return; // F13: zombified piglins born from the portal
        }
        if self.rec_random_tick(state, p) {
            return; // F14: dried ghast
        }
        self.nature_random_tick(state, p);
    }

    pub(crate) fn trigger_event(&mut self, state: u32, p: P, a: i32, b: i32) -> bool {
        match self.v().kind(state) {
            Kind::Piston { sticky } => self.piston_trigger(state, p, a, b, sticky),
            // Bell: survey of nearby entities, shake (resonance at the next tick).
            Kind::Bell => {
                if a == 1 {
                    self.bell_rung(p);
                }
                true
            }
            Kind::NoteBlock => self.note_sound(state, p),
            Kind::Chest => true,
            // G6-F01: wobble of the decorated pot (style 0 or 1).
            Kind::DecoratedPot => a == 1 && (0..2).contains(&b),
            // Lid (opener count) of Ender chests, copper chests, Shulker boxes;
            // the Shulker box also animates its lid on the server side.
            _ if a == 1 => {
                let Some(name) = self.data.block_of(state).map(|b| b.name.as_str()) else { return false };
                if name.ends_with("shulker_box") {
                    self.shulker_lid_event(p, b);
                    return true;
                }
                name.ends_with("ender_chest") || name.ends_with("copper_chest")
            }
            _ => false,
        }
    }

    /// Reaction of a block to a shape update: new state (air = destroyed).
    pub(crate) fn update_shape(&mut self, state: u32, p: P, dir: Dir, np: P, ns: u32) -> u32 {
        // Waterlogged blocks: water tick at each shape (except the water block itself).
        let kind = self.v().kind(state);
        if !matches!(kind, Kind::Water | Kind::Lava) && self.v().get_bool(state, "waterlogged") {
            self.schedule_fluid_tick(p, 1, 5);
        }
        if let Some(s) = self.portal_update_shape(state, p, dir, ns) {
            return s; // S6: Nether portal
        }
        self.rec_update_shape(state, p); // F14: creaking heart
        // Grass, podzol, mycelium: snowy according to the block above (tag `snow`).
        if dir == Dir::Up && self.snowy_block(state) {
            let snowy = self.v().block(ns);
            let snowy =
                self.block_tags.get("minecraft:snow").is_some_and(|t| t.binary_search(&u32::from(snowy)).is_ok());
            return self.v().set_bool(state, "snowy", snowy);
        }
        self.shape_of(state, kind, p, dir, np, ns)
    }

    /// "Snowable" block (grass, podzol, mycelium).
    pub(crate) fn snowy_block(&self, s: u32) -> bool {
        matches!(self.state_name(s), "minecraft:grass_block" | "minecraft:podzol" | "minecraft:mycelium")
    }
}
