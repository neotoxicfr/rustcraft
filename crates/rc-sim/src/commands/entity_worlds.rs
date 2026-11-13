//! Entities of every world for `execute`, like the game's: its entities carry their own world,
//! its selectors search every world unless an option limits them to the source's, and a step
//! reads each entity where it lives. Ours are identifiers read in the world the server works in;
//! one counter numbers the entities of every world (`Server::next_entity`), so an identifier
//! still names one entity across worlds. What this module adds is finding the world holding an
//! entity, searching every open world, and reading each entity found in its own world.
//!
//! A final command other than `tp` still acts on the entities of the world it runs in.

use hashbrown::HashMap;
use rc_proto::nbt::Nbt;

use super::ents::E;
use super::kept_entity_index::KeptEntityIndex;
use super::select::{Sel, parse};
use super::select_candidates::{Record, record_in};
use super::src::{Fail, Src};
use crate::dims::World;
use crate::drops::{ItemEnt, Orb};
use crate::minecart::Carts;
use crate::mobs::Mobs;
use crate::server::{DIMENSIONS, Server};
use crate::tr;

/// The entity lists of one world, the active one's or a parked one's: both answer "does this
/// world hold that entity" through the same test.
pub(crate) struct WorldEntityLists<'a> {
    pub(super) mobs: &'a Mobs,
    pub(super) drops: &'a [ItemEnt],
    pub(super) orbs: &'a [Orb],
    pub(super) kept: &'a HashMap<(i32, i32), Vec<(i32, Nbt)>>,
    pub(super) kept_index: &'a KeptEntityIndex,
    pub(super) carts: &'a Carts,
    /// The entity exploding right now, out of its list.
    pub(super) exploding: Option<&'a (i32, Nbt)>,
}

impl WorldEntityLists<'_> {
    /// Whether this world holds entity `e`; never the player, who is the server's own.
    pub(crate) fn holds(&self, e: E) -> bool {
        record_in(self, e).is_some()
    }

    /// Whether this world may hold any entity at all: a search of every world does not visit a
    /// world whose lists are all empty.
    fn may_hold_any(&self) -> bool {
        self.mobs.list.slots().iter().any(Option::is_some)
            || !self.drops.is_empty()
            || !self.orbs.is_empty()
            || self.kept.values().any(|list| !list.is_empty())
            || !self.mobs.end.snapshots.is_empty()
            || self.carts.list.iter().any(Option::is_some)
            || self.exploding.is_some()
    }
}

impl World {
    /// Whether a ticket holds some of this world's chunks loaded while no player stands in it (a
    /// forced chunk, a pearl, the dragon fight): without one, the game's world has unloaded every
    /// entity, and a search of every world finds none there.
    fn holds_a_ticket(&self) -> bool {
        !self.forced.is_empty()
            || !self.proj.tickets.is_empty()
            || self.mobs.end.fight.as_ref().is_some_and(|f| f.bar_player)
    }

    pub(crate) fn entity_lists(&self) -> WorldEntityLists<'_> {
        WorldEntityLists {
            mobs: &self.mobs,
            drops: &self.drops,
            orbs: &self.orbs,
            kept: &self.entities,
            kept_index: &self.kept_index,
            carts: &self.carts,
            exploding: self.proj.blast_ent.as_ref(),
        }
    }
}

/// Entities an `execute` argument found, in the game's order, with the world holding each.
pub(crate) enum FoundEntities {
    /// All in one world: `None` for the world the server works in, otherwise that open world.
    OneWorld(Option<String>, Vec<E>),
    /// Found in several worlds: each entity with the index of its world in `worlds` (a world
    /// count far below 256).
    SeveralWorlds { entities: Vec<(E, u8)>, worlds: Vec<String> },
}

impl FoundEntities {
    pub(crate) fn len(&self) -> usize {
        match self {
            FoundEntities::OneWorld(_, list) => list.len(),
            FoundEntities::SeveralWorlds { entities, .. } => entities.len(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The first entity found.
    pub(crate) fn first(&self) -> Option<E> {
        match self {
            FoundEntities::OneWorld(_, list) => list.first().copied(),
            FoundEntities::SeveralWorlds { entities, .. } => entities.first().map(|(e, _)| *e),
        }
    }

    /// Keeps the first entity alone (an argument that names one entity).
    fn keep_first(&mut self) {
        match self {
            FoundEntities::OneWorld(_, list) => list.truncate(1),
            FoundEntities::SeveralWorlds { entities, .. } => entities.truncate(1),
        }
    }
}

/// Rank of a world in the game's order of worlds: overworld, Nether, End, then the others by
/// name (the order the world settings list them in, `GenSettings::dimension_names`).
fn game_rank(world: &str) -> (usize, &str) {
    (DIMENSIONS.iter().position(|d| *d == world).unwrap_or(DIMENSIONS.len()), world)
}

impl Server {
    /// The active world's entity lists.
    pub(crate) fn entity_lists(&self) -> WorldEntityLists<'_> {
        WorldEntityLists {
            mobs: &self.mobs,
            drops: &self.life.drops,
            orbs: &self.life.orbs,
            kept: &self.entities,
            kept_index: &self.kept_index,
            carts: &self.carts,
            exploding: self.proj.blast_ent.as_ref(),
        }
    }

    /// The open world holding `e` when it is not the one the server works in: the parked world
    /// whose lists hold it, or for the player the players' world while the server works in
    /// another one. `None`: the world the server works in (or no world at all).
    pub(crate) fn other_world_holding(&self, e: E) -> Option<&str> {
        if e == E::Player {
            return self.players_world_while_away();
        }
        // No other world open: nothing to look for, and no list walked for it.
        if self.dims.parked.is_empty() || self.ent_exists(e) {
            return None;
        }
        self.parked_world_holding(e)
    }

    /// The parked world whose lists hold `e` (never the player).
    fn parked_world_holding(&self, e: E) -> Option<&str> {
        self.dims.parked.iter().find(|w| w.entity_lists().holds(e)).map(|w| w.dim.name.as_str())
    }

    /// Whether chunk `c` of the world the server works in keeps its entities loaded while no
    /// player stands in that world, as the game's tickets hold it at the full-chunk level: a
    /// forced chunk (entity-ticking level) the chunks up to 2 away, a pearl or the dragon fight
    /// theirs. Our parked worlds keep every chunk read in memory; the game's unload the others.
    pub(crate) fn chunk_held_by_a_ticket(&self, c: (i32, i32)) -> bool {
        use crate::mobs::spawn::{ENTITY_TICKING, FULL};
        let forced_reach = FULL - ENTITY_TICKING;
        self.cmd.forced.iter().any(|f| (f.0 - c.0).abs().max((f.1 - c.1).abs()) <= forced_reach)
            || self.ticket_level(c) <= FULL
    }

    /// Runs `read` in the world holding `e` (the world the server works in when it holds it).
    pub(crate) fn in_entity_world<T>(&mut self, e: E, read: impl FnOnce(&mut Server) -> T) -> T {
        match self.other_world_holding(e).map(str::to_owned) {
            Some(world) => self.in_world(&world, read),
            None => read(self),
        }
    }

    /// `read` for each found entity, in the found order, each read in the world holding it: one
    /// visit per world, not one per entity.
    pub(crate) fn read_found<T>(&mut self, found: &FoundEntities, mut read: impl FnMut(&mut Server, E) -> T) -> Vec<T> {
        match found {
            FoundEntities::OneWorld(None, list) => list.iter().map(|&e| read(self, e)).collect(),
            FoundEntities::OneWorld(Some(world), list) => {
                self.in_world(world, |sv| list.iter().map(|&e| read(sv, e)).collect())
            }
            FoundEntities::SeveralWorlds { entities, worlds } => {
                let mut out: Vec<Option<T>> = std::iter::repeat_with(|| None).take(entities.len()).collect();
                for (index, world) in worlds.iter().enumerate() {
                    // A world searched whose entities the sort or the limit left out is not visited.
                    if !entities.iter().any(|&(_, w)| usize::from(w) == index) {
                        continue;
                    }
                    self.in_world(world, |sv| {
                        for (slot, &(e, w)) in out.iter_mut().zip(entities) {
                            if usize::from(w) == index {
                                *slot = Some(read(sv, e));
                            }
                        }
                    });
                }
                out.into_iter().flatten().collect()
            }
        }
    }

    /// Entities of an entity argument of an `execute` step, an empty list allowed.
    pub(crate) fn step_entities(&mut self, src: &Src, text: &str) -> FoundEntities {
        self.step_search(src, text, false).unwrap_or(FoundEntities::OneWorld(None, Vec::new()))
    }

    /// Whether an entity argument of an `execute` condition finds an entity.
    pub(crate) fn step_finds_any(&mut self, src: &Src, text: &str) -> bool {
        self.step_search(src, text, true).is_some_and(|found| !found.is_empty())
    }

    /// Entities of an entity argument of an `execute` step; none is an error.
    pub(crate) fn step_entities_required(&mut self, src: &Src, text: &str) -> Result<FoundEntities, Fail> {
        let Some(found) = self.step_search(src, text, false) else { return Err(Fail(tr!("argument.entity.invalid"))) };
        if found.is_empty() {
            return Err(Fail(tr!("argument.entity.notfound.entity")));
        }
        Ok(found)
    }

    /// [`Server::select_across_worlds`] of an argument's text (`None`: not a selector).
    fn step_search(&mut self, src: &Src, text: &str, first_only: bool) -> Option<FoundEntities> {
        // A plain `@s` (the commonest argument, met once per source) names the source's entity if
        // it still exists: found through the lists of the world holding it, without parsing the
        // text or visiting that world.
        if text == "@s" {
            return Some(self.own_entity(src));
        }
        Some(self.select_across_worlds(&parse(text)?, src, first_only))
    }

    /// The source's entity, if it exists in an open world (a plain `@s`).
    fn own_entity(&self, src: &Src) -> FoundEntities {
        let nothing = FoundEntities::OneWorld(None, Vec::new());
        let Some(e) = src.entity else { return nothing };
        let world = match e {
            E::Player => self.players_world_while_away(),
            _ if self.ent_exists(e) => None,
            _ => match self.parked_world_holding(e) {
                Some(world) => Some(world),
                None => return nothing,
            },
        };
        FoundEntities::OneWorld(world.map(str::to_owned), vec![e])
    }

    /// The one entity an argument of an `execute` step names (`store … entity`, `if data entity`).
    pub(crate) fn step_entity(&mut self, src: &Src, text: &str) -> Result<FoundEntities, Fail> {
        let mut found = self.step_entities_required(src, text)?;
        found.keep_first();
        Ok(found)
    }

    /// Score holders of an `execute` step: a selector's entities named where they live; `*`, a
    /// name or a UUID as for any command.
    pub(crate) fn step_score_holders(&mut self, src: &Src, text: &str) -> Result<Vec<String>, Fail> {
        if !text.starts_with('@') {
            return self.arg_score_holders(src, text);
        }
        if text == "@s"
            && let Some(name) = self.own_score_holder_elsewhere(src)
        {
            return Ok(vec![name]);
        }
        let found = self.step_entities(src, text);
        if found.is_empty() {
            return Err(Fail(tr!("argument.entity.notfound.entity")));
        }
        Ok(self.read_found(&found, |sv, e| sv.ent_score_name(e)))
    }

    /// `read` of the source's entity when a parked world holds it, done on its record in that
    /// world's lists (the selectors' records) rather than in a visit, which costs two world
    /// exchanges per source; with that world's name. `None`: the player, or an entity standing in
    /// the world the server works in, or nowhere.
    fn read_own_record_elsewhere<T>(
        &self,
        src: &Src,
        read: impl FnOnce(&Server, Record<'_>) -> T,
    ) -> Option<(T, String)> {
        let e = src.entity.filter(|e| *e != E::Player)?;
        if self.dims.parked.is_empty() || self.ent_exists(e) {
            return None;
        }
        let (record, world) =
            self.dims.parked.iter().find_map(|w| Some((record_in(&w.entity_lists(), e)?, w.dim.name.as_str())))?;
        Some((read(self, record), world.to_owned()))
    }

    /// Score holder name of the source's entity standing in a parked world, read in place.
    fn own_score_holder_elsewhere(&mut self, src: &Src) -> Option<String> {
        let (name, _) = self.read_own_record_elsewhere(src, |sv, r| crate::nbtio::uuid_text(sv.record_uuid(r)))?;
        #[cfg(test)]
        if crate::mobs::index::verify()
            && let Some(e) = src.entity
        {
            let visited = self.in_entity_world(e, |sv| sv.ent_score_name(e));
            assert_eq!(name, visited, "score holder read in place differs from the one read in its world");
        }
        Some(name)
    }

    /// Where the source's entity standing in a parked world stands (`execute at @s`): position,
    /// rotation (pitch, yaw) and world, read in place.
    pub(crate) fn own_place_elsewhere(&mut self, src: &Src) -> Option<([f64; 3], [f32; 2], String)> {
        let ((pos, [yaw, pitch]), world) =
            self.read_own_record_elsewhere(src, |sv, r| (sv.record_pos(r), sv.record_rot(r)))?;
        #[cfg(test)]
        if crate::mobs::index::verify()
            && let Some(e) = src.entity
        {
            let visited = self.in_entity_world(e, |sv| (sv.e_pos(e), sv.ent_rot(e), sv.dim.name.clone()));
            assert_eq!((pos, [yaw, pitch], world.clone()), visited, "place read in place differs from its world's");
        }
        Some((pos, [pitch, yaw], world))
    }

    /// The game's search of a selector: a player name or the player list from every world (a
    /// world-limited one keeps the players standing in the source's world), a UUID in every
    /// world, `@s` where the source's entity lives whatever its options, a world-limited
    /// selector in the source's world, any other in every world. `first_only`: the caller only
    /// asks whether one matches.
    pub(crate) fn select_across_worlds(&mut self, s: &Sel, src: &Src, first_only: bool) -> FoundEntities {
        if s.name.is_some() || s.players_only {
            let found = self.select_up_to(s, src, first_only);
            return FoundEntities::OneWorld(self.players_world_while_away().map(str::to_owned), found);
        }
        if let Some(u) = s.uuid {
            return self.find_uuid_in_every_world(u);
        }
        let world = if s.current {
            src.entity.and_then(|e| self.other_world_holding(e)).map(str::to_owned)
        } else if s.world_limited {
            (src.dim != self.dim.name).then(|| src.dim.clone())
        } else {
            return self.select_in_every_world(s, src, first_only);
        };
        let found = match &world {
            Some(w) => self.in_world(w, |sv| sv.search_here(s, src, first_only)),
            None => self.search_here(s, src, first_only),
        };
        FoundEntities::OneWorld(world, found)
    }

    /// The entity carrying UUID `u`, among the entities the game would have loaded: the world the
    /// server works in first (no exchange), then the other worlds a search of every world visits,
    /// in the game's order (a UUID names one entity, so the order only costs visits). A parked
    /// world without a ticket is not visited, like for any search of every world.
    fn find_uuid_in_every_world(&mut self, u: u128) -> FoundEntities {
        if let Some(e) = self.loaded_entity_by_uuid(u) {
            let world = if e == E::Player { self.players_world_while_away().map(str::to_owned) } else { None };
            return FoundEntities::OneWorld(world, vec![e]);
        }
        if self.dims.parked.is_empty() {
            return FoundEntities::OneWorld(None, Vec::new());
        }
        for world in self.worlds_to_search() {
            if world == self.dim.name {
                continue;
            }
            if let Some(e) = self.in_world(&world, |sv| sv.loaded_entity_by_uuid(u)) {
                return FoundEntities::OneWorld(Some(world), vec![e]);
            }
        }
        FoundEntities::OneWorld(None, Vec::new())
    }

    /// The entity of the world the server works in carrying UUID `u`, if the game would have it
    /// loaded: in a world nobody stands in (a LAN guest counts), only in a chunk a ticket holds (the
    /// same rule as the selectors searching every world). The player stands in their own world.
    fn loaded_entity_by_uuid(&self, u: u128) -> Option<E> {
        let e = self.ent_by_uuid(u)?;
        let loaded = e.is_player()
            || self.someone_stands_in(&self.dim.name)
            || self.chunk_held_by_a_ticket(crate::decoration::chunk_of(self.e_pos(e)));
        loaded.then_some(e)
    }

    /// A selector that no option limits to one world, searched in every open world in the game's
    /// order, then sorted and limited once over all of them, like the game: an arbitrary order
    /// stops at the limit (the overworld's entities first), `@n` takes the nearest coordinates
    /// whatever the world.
    fn select_in_every_world(&mut self, s: &Sel, src: &Src, first_only: bool) -> FoundEntities {
        let worlds = if self.dims.parked.is_empty() { Vec::new() } else { self.worlds_to_search() };
        // One world holding entities: its search, as before.
        if worlds.len() <= 1 {
            return FoundEntities::OneWorld(None, self.search_here(s, src, first_only));
        }
        let base = super::select::selector_base(s, src);
        let stop_at_first = first_only && self.cmd.paths.execute_if_entity_first_match;
        let budget = super::select_candidates::match_budget(s, stop_at_first);
        let mut matches = Vec::new();
        for (index, world) in worlds.iter().enumerate() {
            if matches.len() >= budget {
                break;
            }
            let left = budget - matches.len();
            let part = self.in_world(world, |sv| sv.matches_before_sort(s, src, base, left));
            matches.extend(part.into_iter().map(|(e, d2)| ((e, index as u8), d2)));
        }
        let entities = self.order_and_limit(s, matches);
        FoundEntities::SeveralWorlds { entities, worlds }
    }

    /// The open worlds a search of every world visits, in the game's order: the world the
    /// server works in, the worlds a player stands in (the current one's, a LAN guest's), and
    /// each parked world that may hold a loaded entity (one with no entity, or with no ticket
    /// holding a chunk, costs no visit).
    fn worlds_to_search(&self) -> Vec<String> {
        let mut worlds: Vec<&str> = Vec::with_capacity(1 + self.dims.parked.len());
        worlds.push(&self.dim.name);
        for w in &self.dims.parked {
            if self.someone_stands_in(&w.dim.name) || (w.holds_a_ticket() && w.entity_lists().may_hold_any()) {
                worlds.push(&w.dim.name);
            }
        }
        worlds.sort_by(|a, b| game_rank(a).cmp(&game_rank(b)));
        worlds.into_iter().map(str::to_owned).collect()
    }
}
