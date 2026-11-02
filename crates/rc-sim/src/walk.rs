//! Entity walk of the world tick (§ 1.2, step 11b), in one order for every kind of entity.
//!
//! The game keeps, per world, a single list of the entities that tick, in the order they joined
//! it: an entity joins at the end when it starts ticking (added in a "ticking entities" chunk, or
//! its chunk becomes one), leaves it when it stops (its chunk stops ticking, or it moves into a
//! chunk that does not tick) and joins at the end again when it comes back. The walk visits that
//! list once, every kind together, players included; what joins during the walk waits for the
//! next one.
//!
//! Here every family keeps its own list (falling blocks and TNT, items, orbs, mobs…), each in
//! the order of its entities' ranks: the rank is stamped when an entity enters its list. The walk
//! merges the families by rank (a k-way merge of lists already in order, by runs: no allocation
//! in steady state), visiting each entity through its family's own tick. The phases a family runs
//! for all its entities at once (network tracking before, item merge, pickup, removals after)
//! stay around the merged walk.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::server::Server;

/// Families of entities, each with its own list (`Server::fam_len`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Fam {
    /// Falling blocks and primed TNT (`bs.entities.list`).
    Sim,
    /// The host player: the part of its tick that belongs to the world (mining progress, demo).
    Player,
    Item,
    Orb,
    Mob,
    /// Evoker fangs.
    Fang,
    /// Llama spits.
    Spit,
    /// Lightning bolts.
    Bolt,
    /// End crystals.
    Crystal,
    /// Ominous item spawners.
    Ominous,
    /// Paintings, frames, armor stands, cushions, leash knots.
    Deco,
    Cart,
    /// Arrows, thrown items, fireballs, bullets, wind charges…
    Proj,
    /// Area effect clouds.
    Cloud,
    /// The host's fishing bobber.
    Hook,
}

pub(crate) const FAMS: [Fam; 15] = [
    Fam::Sim,
    Fam::Player,
    Fam::Item,
    Fam::Orb,
    Fam::Mob,
    Fam::Fang,
    Fam::Spit,
    Fam::Bolt,
    Fam::Crystal,
    Fam::Ominous,
    Fam::Deco,
    Fam::Cart,
    Fam::Proj,
    Fam::Cloud,
    Fam::Hook,
];

/// Bit of a family in a walk mask.
pub(crate) const fn bit(f: Fam) -> u16 {
    1 << f as u16
}

/// Every family (the server tick).
pub(crate) const ALL: u16 = (1 << FAMS.len()) - 1;

/// Families still visited when the game is frozen (`tick freeze`): the player, and its mount
/// (a mob or a minecart).
pub(crate) const FROZEN: u16 = bit(Fam::Player) | bit(Fam::Mob) | bit(Fam::Cart);

/// Rank counter. Only the order of two ranks of the same server matters, so a single counter
/// for the process is enough (several servers in parallel tests only leave gaps).
static RANKS: AtomicU64 = AtomicU64::new(0);

/// Rank of an entity entering its list now: after every entity already in a list. Ranks are
/// even; an odd rank (`rank | 1`) marks an entity that left the list (outside a "ticking
/// entities" chunk) and keeps its place among the others until it comes back.
pub(crate) fn next_rank() -> u64 {
    RANKS.fetch_add(2, Ordering::Relaxed) + 2
}

/// What the walk does with an entity it meets at its rank.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Meet {
    /// In a ticking chunk and in the list: it ticks now.
    Tick,
    /// Outside a ticking chunk: it waits (and leaves the list).
    Wait,
    /// Back in a ticking chunk: it joined the end of the list, it ticks at the end of the walk.
    Late,
}

#[derive(Debug, Default)]
pub(crate) struct Walk {
    /// A walk is in progress (`snap` is valid).
    on: bool,
    /// Length of each family's list at the start of the walk: what is added afterwards waits for
    /// the next walk.
    snap: [u32; FAMS.len()],
    /// Entities back in a ticking chunk during this walk, with their new rank: ticked once the
    /// merged walk is over, in that order.
    late: Vec<(Fam, u32)>,
    /// Families where an entity got a new rank (list sorted again at the end of the walk).
    resort: u16,
    /// Rank of the host player in its world's list (renewed at respawn and dimension change).
    pub(crate) player: u64,
    /// Placeholders put in the slot of the item or orb being ticked (the list stays in place).
    pub(crate) spare_item: Option<crate::drops::ItemEnt>,
    pub(crate) spare_orb: Option<crate::drops::Orb>,
    /// Orb merge buckets built for this walk (`drops::orb_buckets`).
    pub(crate) buckets_built: bool,
}

impl Walk {
    /// A walk is in progress: an entity added now waits for the next one.
    pub(crate) fn on(&self) -> bool {
        self.on
    }
}

impl Server {
    /// Start of an entity walk, if none is in progress: snapshot of the lists. True when started
    /// here (the caller then ends it with [`Server::walk_end`]).
    pub(crate) fn walk_begin(&mut self) -> bool {
        if self.walk.on {
            return false;
        }
        for (k, &f) in FAMS.iter().enumerate() {
            self.walk.snap[k] = self.fam_len(f) as u32;
        }
        self.walk.on = true;
        self.walk.buckets_built = false;
        true
    }

    /// End of the walk started by `walk_begin` (`own`): lists where an entity was given a new
    /// rank are put back in rank order.
    pub(crate) fn walk_end(&mut self, own: bool) {
        if !own {
            return;
        }
        let resort = std::mem::take(&mut self.walk.resort);
        if resort != 0 {
            self.fam_resort(resort);
        }
        self.walk.late.clear();
        self.walk.on = false;
    }

    /// Meeting of an entity (family `f`, slot `i`) by the walk; `ticking`: its chunk is "ticking
    /// entities". Outside such a chunk it leaves the list (odd rank); back in one, it joins the
    /// end of the list (new rank) and its tick waits for the end of the walk.
    pub(crate) fn meet(&mut self, rank: &mut u64, ticking: bool, f: Fam, i: usize) -> Meet {
        if !ticking {
            *rank |= 1;
            return Meet::Wait;
        }
        if *rank & 1 == 0 {
            return Meet::Tick;
        }
        *rank = next_rank();
        self.walk.resort |= bit(f);
        if !self.walk.on {
            return Meet::Tick;
        }
        self.walk.late.push((f, i as u32));
        Meet::Late
    }

    /// The merged walk over the families of `mask`, by rank: each step picks the family whose
    /// next entity has the smallest rank and ticks its entities while they stay ahead of every
    /// other family's next one. Only the families of `mask` that have entities are kept, with
    /// the rank of their next entity: a step reads the rank of the family that moved, never the
    /// others' (a mob's rank sits behind a pointer). Stops if the world changes under it (player
    /// sent to another dimension by a command run during the walk).
    pub(crate) fn walk_run(&mut self, mask: u16) {
        let world = self.dim.name.as_ptr();
        // (family, next slot, end of the snapshot, rank of the next slot).
        let mut heads = [(Fam::Sim, 0u32, 0u32, 0u64); FAMS.len()];
        let mut n = 0;
        for (k, &f) in FAMS.iter().enumerate() {
            let end = self.walk.snap[k];
            if mask & bit(f) != 0
                && end > 0
                && let Some(r) = self.fam_rank(f, 0)
            {
                heads[n] = (f, 0, end, r);
                n += 1;
            }
        }
        while n > 0 {
            // Smallest next rank, and the smallest among the other families.
            let (mut best, mut limit) = (0, u64::MAX);
            for j in 1..n {
                let r = heads[j].3;
                if r < heads[best].3 {
                    limit = heads[best].3;
                    best = j;
                } else if r < limit {
                    limit = r;
                }
            }
            let (f, start, end, _) = heads[best];
            // Items and orbs tick a whole run at once (`item_run`); the others one by one.
            let i = match f {
                Fam::Item => self.item_run(start as usize, end as usize, limit) as u32,
                Fam::Orb => self.orb_run(start as usize, end as usize, limit) as u32,
                _ => {
                    let mut i = start;
                    loop {
                        self.fam_visit(f, i as usize);
                        i += 1;
                        let ahead = |s: &Server, i: u32| s.fam_rank(f, i as usize).is_some_and(|r| r < limit);
                        if i >= end || self.dim.name.as_ptr() != world || !ahead(self, i) {
                            break i;
                        }
                    }
                }
            };
            if self.dim.name.as_ptr() != world {
                self.walk.late.clear();
                return;
            }
            let next = if i < end { self.fam_rank(f, i as usize) } else { None };
            match next {
                Some(r) => heads[best] = (f, i, end, r),
                None => {
                    n -= 1;
                    heads[best] = heads[n];
                }
            }
        }
        // Entities back in a ticking chunk: at the end of the list.
        let mut late = std::mem::take(&mut self.walk.late);
        for &(f, i) in &late {
            if self.dim.name.as_ptr() != world {
                break;
            }
            self.fam_visit(f, i as usize);
        }
        late.clear();
        self.walk.late = late;
    }

    /// One family alone, as its own walk if none is in progress (block probes, tests, parked
    /// worlds): its phases before, its entities, its phases after.
    #[cfg(test)]
    pub(crate) fn walk_alone(&mut self, mask: u16, pre: impl FnOnce(&mut Server), post: impl FnOnce(&mut Server)) {
        let own = self.walk_begin();
        pre(self);
        self.walk_run(mask);
        post(self);
        self.walk_end(own);
    }

    /// Length of a family's list.
    fn fam_len(&self, f: Fam) -> usize {
        match f {
            Fam::Sim => self.bs.entities.list.len(),
            Fam::Player => 1,
            Fam::Item => self.life.drops.len(),
            Fam::Orb => self.life.orbs.len(),
            Fam::Mob => self.mobs.list.len(),
            Fam::Fang => self.mobs.ill.fangs.len(),
            Fam::Spit => self.mobs.mounts.spits.len(),
            Fam::Bolt => self.mobs.mounts.bolts.len(),
            Fam::Crystal => self.mobs.end.crystals.len(),
            Fam::Ominous => self.ominous_len(),
            Fam::Deco => self.deco_live_len(),
            Fam::Cart => self.carts.list.len(),
            Fam::Proj => self.proj.list.len(),
            Fam::Cloud => self.proj.clouds.len(),
            Fam::Hook => usize::from(self.proj.hook.is_some()),
        }
    }

    /// Rank of the entity in slot `i` of a family (`None`: no such slot); an empty slot (entity
    /// taken out) counts as the very first.
    #[inline]
    fn fam_rank(&self, f: Fam, i: usize) -> Option<u64> {
        Some(match f {
            Fam::Sim => self.bs.entities.list.get(i)?.rank,
            Fam::Player => self.walk.player,
            Fam::Item => self.life.drops.get(i)?.rank,
            Fam::Orb => self.life.orbs.get(i)?.rank,
            Fam::Mob => self.mobs.list.get(i)?.as_ref().map_or(0, |m| m.rank),
            Fam::Fang => self.mobs.ill.fangs.get(i)?.rank,
            Fam::Spit => self.mobs.mounts.spits.get(i)?.rank,
            Fam::Bolt => self.mobs.mounts.bolts.get(i)?.rank,
            Fam::Crystal => self.mobs.end.crystals.get(i)?.rank,
            Fam::Ominous => self.ominous_rank(i)?,
            Fam::Deco => self.deco_rank(i)?,
            Fam::Cart => self.carts.list.get(i)?.as_ref().map_or(0, |c| c.rank),
            Fam::Proj => self.proj.list.get(i)?.rank,
            Fam::Cloud => self.proj.clouds.get(i)?.rank,
            Fam::Hook => self.proj.hook.as_ref()?.p.rank,
        })
    }

    /// Tick of the entity in slot `i` of a family, at its rank.
    #[inline]
    fn fam_visit(&mut self, f: Fam, i: usize) {
        match f {
            Fam::Sim => self.sim_visit(i),
            Fam::Player => self.player_world_tick(),
            Fam::Item => self.item_visit(i),
            Fam::Orb => self.orb_visit(i),
            Fam::Mob => self.mob_visit(i),
            Fam::Fang => self.fang_visit(i),
            Fam::Spit => self.spit_visit(i),
            Fam::Bolt => self.bolt_visit(i),
            Fam::Crystal => self.crystal_visit(i),
            Fam::Ominous => self.ominous_visit(i),
            Fam::Deco => self.deco_visit(i),
            Fam::Cart => self.cart_visit(i),
            Fam::Proj => self.proj_visit(i),
            Fam::Cloud => self.cloud_visit(i),
            Fam::Hook => self.fishing_tick(),
        }
    }

    /// Lists of `mask` put back in rank order (stable: entities that left the list keep their
    /// place), after a walk where some of their entities joined them again.
    fn fam_resort(&mut self, mask: u16) {
        let has = |f: Fam| mask & bit(f) != 0;
        if has(Fam::Sim) {
            self.bs.entities.list.sort_by_key(|e| e.rank);
        }
        if has(Fam::Item) {
            self.life.drops.sort_by_key(|d| d.rank);
        }
        if has(Fam::Orb) {
            self.life.orbs.sort_by_key(|o| o.rank);
        }
        if has(Fam::Mob) {
            self.mobs.list.sort_by_key(|m| m.as_ref().map_or(0, |m| m.rank));
            self.mobs.index.mark_stale();
        }
        if has(Fam::Fang) {
            self.mobs.ill.fangs.sort_by_key(|f| f.rank);
        }
        if has(Fam::Spit) {
            self.mobs.mounts.spits.sort_by_key(|s| s.rank);
        }
        if has(Fam::Bolt) {
            self.mobs.mounts.bolts.sort_by_key(|b| b.rank);
        }
        if has(Fam::Crystal) {
            self.mobs.end.crystals.sort_by_key(|c| c.rank);
        }
        if has(Fam::Ominous) {
            self.ominous_resort();
        }
        if has(Fam::Deco) {
            self.deco_resort();
        }
        if has(Fam::Cart) {
            self.carts.list.sort_by_key(|c| c.as_ref().map_or(0, |c| c.rank));
        }
        if has(Fam::Proj) {
            self.proj.list.sort_by_key(|p| p.rank);
        }
        if has(Fam::Cloud) {
            self.proj.clouds.sort_by_key(|c| c.rank);
        }
    }

    /// The host player's own part of the world walk, at its rank: progress of the block being
    /// mined (and the delayed break), then the demo messages.
    fn player_world_tick(&mut self) {
        self.tick_mining();
        self.demo_tick();
    }

    /// Entity phase of the server tick: phases run for a whole family before the walk, the
    /// merged walk, then the phases after it. Frozen (`tick freeze`), only the player and its
    /// mount tick; the player's own phases still run.
    pub(crate) fn tick_entities(&mut self) {
        #[cfg(test)]
        crate::server::phase("entities");
        let runs = self.cmd.ticking.runs();
        // A player's pearl read back from its file (on entry) is there before the walk.
        self.proj_restore_pearls();
        let own = self.walk_begin();
        // Before the walk: network tracking (the client receives the state of the previous
        // tick), the player's parts that come first.
        if runs {
            self.sim_track();
            self.drops_track();
        }
        self.mobs_pre();
        self.cushion_seat_tick(); // seated player: it ticks even when the world is frozen
        self.carts_track();
        self.proj_pre(runs);
        self.walk_run(if runs { ALL } else { FROZEN });
        // After the walk: removals, merges, pickups, the player's own phases.
        if runs {
            self.sim_post();
            self.drops_post();
        }
        self.life_player_tick();
        self.mobs_post(runs);
        if runs {
            self.deco_post();
        }
        self.carts_post();
        self.proj_post(runs);
        self.walk_end(own);
    }

    /// Entity walk of a parked world, player absent: under a portal ticket its falling blocks,
    /// TNT, items, orbs and projectiles; under a player's pearl ticket alone, its projectiles.
    pub(crate) fn tick_parked_entities(&mut self, portal: bool) {
        let own = self.walk_begin();
        if portal {
            self.sim_track();
            self.drops_track();
        }
        self.proj_track();
        self.load_pearl_chunks();
        let fams = bit(Fam::Sim) | bit(Fam::Item) | bit(Fam::Orb);
        self.walk_run(bit(Fam::Proj) | if portal { fams } else { 0 });
        if portal {
            self.sim_post();
            self.drops_post();
            self.drops_portal_tick();
        }
        self.proj_retain();
        self.walk_end(own);
    }
}
