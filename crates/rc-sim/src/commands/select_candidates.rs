//! Selector search over candidates resolved once (`RUSTCRAFT_OPT_SELECTOR_CANDIDATE_VIEW`).
//!
//! The former search listed every entity by identifier, then every option looked the entity up
//! again by that identifier (type, position, rotation, tags, box: a walk of a list each, so a
//! search was quadratic in the number of entities) and built a `String` or a `Vec` per option.
//! Here the walk of the lists hands each candidate with the record it was found in: every option
//! reads it in place. Per selector, what does not depend on the entity is prepared once (type tag
//! members, the translated default names). Same entities, same order, same draws on the world's
//! random as [`Server::select_reference`] (checked against it at every search during tests).
//!
//! `RUSTCRAFT_OPT_SELECTOR_TYPE_FIRST`: `type=<id>` walks only the entities of that type.

use std::borrow::Cow;

use rc_proto::nbt::Nbt;

use super::entity_worlds::WorldEntityLists;
use super::ents::E;
use super::select::{Sel, Sort, java_eq_ignore_case, nbt_matches, rot_test};
use super::src::Src;
use crate::drops::{ItemEnt, Orb};
use crate::minecart::Cart;
use crate::mobs::mob::Mob;
use crate::mobs::species::Kind;
use crate::nbtio;
use crate::server::Server;

/// Where a candidate's data lives, read in place.
#[derive(Clone, Copy)]
pub(super) enum Record<'a> {
    Player,
    Mob(&'a Mob),
    Item(&'a ItemEnt),
    Orb(&'a Orb),
    /// Kept compound: chunk lists, End family snapshots, the entity exploding right now.
    Kept(&'a Nbt),
    Cart(&'a Cart),
}

/// One entity a selector may return, with its record.
#[derive(Clone, Copy)]
struct Candidate<'a> {
    e: E,
    record: Record<'a>,
}

/// `type=` test prepared for a selector.
enum TypeTest<'a> {
    Exact(&'a str),
    /// Tag members, under the tag's name without and with its namespace (both, like the former
    /// test).
    Members(&'a [String], &'a [String]),
}

impl TypeTest<'_> {
    fn hit(&self, ty: &str) -> bool {
        match self {
            TypeTest::Exact(t) => *t == ty,
            TypeTest::Members(a, b) => a.iter().chain(b.iter()).any(|m| m == ty),
        }
    }
}

/// What a selector's options need, computed once per search.
struct Prepared<'a> {
    types: Vec<(bool, TypeTest<'a>)>,
    /// Translated default name per entity type (option `name=` only).
    default_names: Vec<(&'a str, String)>,
    /// The distance is needed: option `distance` or a nearest/furthest sort.
    wants_distance: bool,
}

impl Server {
    /// Selected entities in the game's order, each candidate read from its record. `first_only`:
    /// the caller only asks whether one matches (`execute if entity`), and the list may then hold
    /// the first match alone.
    pub(crate) fn select_resolved(&mut self, s: &Sel, src: &Src, first_only: bool) -> Vec<E> {
        if let Some(n) = &s.name {
            return if java_eq_ignore_case(n, &self.name) { vec![E::Player] } else { Vec::new() };
        }
        if let Some(u) = s.uuid {
            return self.ent_by_uuid(u).into_iter().collect();
        }
        let base = super::select::selector_base(s, src);
        if s.current {
            let found = match src.entity.and_then(|e| self.candidate_of(e)) {
                Some(c) => {
                    let mut prep = self.prepare_selector(s);
                    self.candidate_passes(s, &mut prep, &c, base).map(|_| c.e).into_iter().collect()
                }
                None => Vec::new(),
            };
            #[cfg(test)]
            self.check_against_reference(s, src, &found);
            return found;
        }
        let found = self.matching_candidates(s, base, match_budget(s, first_only), self.player_searched(s));
        #[cfg(test)]
        if s.sort == Sort::Random {
            let before_shuffle: Vec<E> = found.iter().map(|(e, _)| *e).collect();
            self.check_matches_against_reference(s, src, base, self.player_searched(s), &before_shuffle);
        }
        let found = self.order_and_limit(s, found);
        #[cfg(test)]
        if s.sort != Sort::Random && !first_only {
            self.check_against_reference(s, src, &found);
        }
        found
    }

    /// The game's order over the matches, then the limit: stable sorts on the distance computed
    /// once (the same order as the former sorts, which computed it again at every comparison;
    /// the game's list sort is stable too), or the shuffle on the world's random.
    pub(super) fn order_and_limit<T>(&mut self, s: &Sel, mut found: Vec<(T, f64)>) -> Vec<T> {
        match s.sort {
            Sort::Arbitrary | Sort::Random => {}
            Sort::Nearest => found.sort_by(|a, b| a.1.total_cmp(&b.1)),
            Sort::Furthest => found.sort_by(|a, b| b.1.total_cmp(&a.1)),
        }
        let mut found: Vec<T> = found.into_iter().map(|(e, _)| e).collect();
        if s.sort == Sort::Random {
            // Fisher-Yates shuffle on the world's random, as the former path.
            for i in (1..found.len()).rev() {
                let j = self.life.rng.next_int_bound((i + 1) as i32) as usize;
                found.swap(i, j);
            }
        }
        found.truncate(s.limit);
        found
    }

    /// This world's matches for a search of every world, before the sort: at most `budget`, each
    /// with its squared distance to `base`, the player only in the world they stand in (met
    /// once over all the worlds). In a world no player stands in, only the entities of the
    /// chunks a ticket holds count ([`Server::chunk_held_by_a_ticket`]).
    pub(super) fn matches_before_sort(&self, s: &Sel, src: &Src, base: [f64; 3], budget: usize) -> Vec<(E, f64)> {
        if self.player_stands_here() {
            return self.matches_up_to(s, src, base, budget, true);
        }
        // The held chunks filter the matches after the search, which then may not stop at the
        // budget.
        let mut found = self.matches_up_to(s, src, base, usize::MAX, false);
        found.retain(|(e, _)| self.chunk_held_by_a_ticket(crate::decoration::chunk_of(self.e_pos(*e))));
        found.truncate(budget);
        found
    }

    /// The matches of this world in the listing order, at most `budget`, each with its squared
    /// distance to `base`: the candidate view, or the former search (switch off).
    fn matches_up_to(&self, s: &Sel, src: &Src, base: [f64; 3], budget: usize, player_searched: bool) -> Vec<(E, f64)> {
        if self.cmd.paths.selector_candidate_view {
            let found = self.matching_candidates(s, base, budget, player_searched);
            #[cfg(test)]
            self.check_world_matches_against_reference(s, src, base, budget, player_searched, &found);
            return found;
        }
        let mut found = self.reference_matches(s, src, base, player_searched);
        found.truncate(budget);
        found
            .into_iter()
            .map(|e| {
                let p = self.e_pos(e);
                (e, (p[0] - base[0]).powi(2) + (p[1] - base[1]).powi(2) + (p[2] - base[2]).powi(2))
            })
            .collect()
    }

    /// Candidates that pass every option, in the listing order, at most `stop_at`, with their
    /// squared distance to `base` (0 when nothing needs it); the player only when
    /// `player_searched`.
    fn matching_candidates(&self, s: &Sel, base: [f64; 3], stop_at: usize, player_searched: bool) -> Vec<(E, f64)> {
        let candidates = if s.players_only {
            let dead_hidden = s.letter != Some('a') && s.letter != Some('p') && s.letter != Some('r');
            if self.life.vit.dead && dead_hidden { Vec::new() } else { vec![Candidate { e: E::Player, record: Record::Player }] }
        } else {
            self.listed_candidates(self.type_walked_alone(s))
        };
        let mut prep = self.prepare_selector(s);
        let mut found = Vec::new();
        for c in &candidates {
            if found.len() >= stop_at {
                break;
            }
            if c.e == E::Player && !player_searched {
                continue;
            }
            if let Some(d2) = self.candidate_passes(s, &mut prep, c, base) {
                found.push((c.e, d2));
            }
        }
        found
    }

    /// The one entity type to walk alone (`type=<id>` without `!` or `#`), when the switch is on.
    fn type_walked_alone<'s>(&self, s: &'s Sel) -> Option<&'s str> {
        if !self.cmd.paths.selector_type_first {
            return None;
        }
        s.types.iter().find(|(neg, _, is_tag)| !neg && !is_tag).map(|(_, t, _)| t.as_str())
    }

    /// Every living entity in the order of [`Server::all_ents`] (player, then by identifier,
    /// earlier family first on a tie), or only those of type `only`.
    fn listed_candidates(&self, only: Option<&str>) -> Vec<Candidate<'_>> {
        let mut out = Vec::new();
        if !self.life.vit.dead && only.is_none_or(|t| t == "minecraft:player") {
            out.push(Candidate { e: E::Player, record: Record::Player });
        }
        let first_other = out.len();
        self.push_mobs(only, &mut out);
        if only.is_none_or(|t| t == "minecraft:item") {
            out.extend(self.life.drops.iter().map(|d| Candidate { e: E::Item(d.id), record: Record::Item(d) }));
        }
        if only.is_none_or(|t| t == "minecraft:experience_orb") {
            out.extend(self.life.orbs.iter().map(|o| Candidate { e: E::Orb(o.id), record: Record::Orb(o) }));
        }
        let kept_of_type = |n: &Nbt| only.is_none_or(|t| kept_type(n) == t);
        let kept = self.entities.values().flatten().chain(&self.mobs.end.snapshots);
        out.extend(kept.filter(|(_, n)| kept_of_type(n)).map(|(id, n)| Candidate { e: E::Other(*id), record: Record::Kept(n) }));
        let carts = self.carts.list.iter().flatten().filter(|c| !c.removed && only.is_none_or(|t| c.kind.name() == t));
        out.extend(carts.map(|c| Candidate { e: E::Other(c.id), record: Record::Cart(c) }));
        // Stable: on a shared identifier the earlier family stays first, like `all_ents`.
        out[first_other..].sort_by_key(|c| self.ent_id(c.e));
        out
    }

    /// Living mobs in the list's order; of type `only`: through the per-species index when one
    /// species carries that name.
    fn push_mobs<'a>(&'a self, only: Option<&str>, out: &mut Vec<Candidate<'a>>) {
        let as_candidate = |m: &'a Mob| Candidate { e: E::Mob(m.id), record: Record::Mob(m) };
        let Some(t) = only else {
            out.extend(self.mobs.list.iter().filter(|m| m.alive()).map(as_candidate));
            return;
        };
        let mut kinds = Kind::ALL.into_iter().filter(|k| k.info().name == t);
        match (kinds.next(), kinds.next()) {
            (None, _) => {}
            (Some(k), None) => out.extend(self.mobs.of_kind(k).filter(|m| m.alive()).map(as_candidate)),
            _ => out.extend(self.mobs.list.iter().filter(|m| m.alive() && m.kind.info().name == t).map(as_candidate)),
        }
    }

    /// Record of an existing entity (`@s`), as [`Server::ent_exists`] finds it.
    fn candidate_of(&self, e: E) -> Option<Candidate<'_>> {
        let record = match e {
            E::Player => Record::Player,
            _ => record_in(&self.entity_lists(), e)?,
        };
        Some(Candidate { e, record })
    }

    fn prepare_selector<'a>(&'a self, s: &'a Sel) -> Prepared<'a> {
        let tag = |t: &'a str| self.cmd.data.tag_members("minecraft:entity_type", t);
        let types = s
            .types
            .iter()
            .map(|(neg, t, is_tag)| {
                let test = if *is_tag {
                    TypeTest::Members(tag(t.trim_start_matches("minecraft:")), tag(t))
                } else {
                    TypeTest::Exact(t.as_str())
                };
                (*neg, test)
            })
            .collect();
        let wants_distance = s.distance.is_some() || matches!(s.sort, Sort::Nearest | Sort::Furthest);
        Prepared { types, default_names: Vec::new(), wants_distance }
    }

    /// Every option of `s` on `c`, cheap ones first (the order is unobservable: only the data
    /// pack predicates draw, on their own local random source); `Some(squared distance)` when it
    /// passes.
    fn candidate_passes<'a>(&'a self, s: &Sel, prep: &mut Prepared<'a>, c: &Candidate<'a>, base: [f64; 3]) -> Option<f64> {
        let e = c.e;
        if s.players_only && e != E::Player {
            return None;
        }
        if !prep.types.is_empty() {
            let ty = self.record_type(c.record);
            if prep.types.iter().any(|(neg, test)| test.hit(ty) == *neg) {
                return None;
            }
        }
        let d2 = if prep.wants_distance {
            let p = self.record_pos(c.record);
            (p[0] - base[0]).powi(2) + (p[1] - base[1]).powi(2) + (p[2] - base[2]).powi(2)
        } else {
            0.0
        };
        if let Some(r) = &s.distance
            && !(r.min.is_none_or(|m| d2 >= m * m) && r.max.is_none_or(|m| d2 <= m * m))
        {
            return None;
        }
        if s.d.iter().any(Option::is_some) && !self.record_in_box(s, c.record, base) {
            return None;
        }
        (self.record_passes_data(s, c) && self.record_passes_name(s, prep, c) && self.record_passes_costly(s, e))
            .then_some(d2)
    }

    /// Options `dx`/`dy`/`dz`: the box from `base` overlaps the entity's box.
    fn record_in_box(&self, s: &Sel, r: Record<'_>, base: [f64; 3]) -> bool {
        let mut min = [0.0; 3];
        let mut max = [0.0; 3];
        for k in 0..3 {
            let d = s.d[k].unwrap_or(0.0);
            min[k] = base[k] + d.min(0.0);
            max[k] = base[k] + d.max(0.0) + 1.0;
        }
        let b = self.record_bbox(r);
        (0..3).all(|k| b.min[k] < max[k] && b.max[k] > min[k])
    }

    /// Tags, team, game mode, level, rotations, scores.
    fn record_passes_data(&self, s: &Sel, c: &Candidate<'_>) -> bool {
        let e = c.e;
        if !s.tags.is_empty() {
            // Compared on the borrowed list (the former test copied every tag into a `Vec`).
            let tags = nbtio::list(self.record_tags(c.record));
            let names = || tags.iter().filter_map(Nbt::as_str);
            for (neg, t) in &s.tags {
                let hit = if t.is_empty() { names().next().is_none() } else { names().any(|x| x == t) };
                if hit == *neg {
                    return false;
                }
            }
        }
        if !s.teams.is_empty() {
            let team = match c.record {
                Record::Player => self.cmd.scores.team_of(&self.name),
                r => self.cmd.scores.team_of_uuid(self.record_uuid(r)),
            }
            .map(|t| t.name.as_str());
            for (neg, t) in &s.teams {
                let hit = if t.is_empty() { team.is_none() } else { team == Some(t.as_str()) };
                if hit == *neg {
                    return false;
                }
            }
        }
        for (neg, m) in &s.modes {
            if e != E::Player || (self.mode_name() == m) == *neg {
                return false;
            }
        }
        if let Some(l) = &s.level
            && (e != E::Player || !l.test(f64::from(self.life.vit.xp_level)))
        {
            return false;
        }
        if s.x_rot.is_some() || s.y_rot.is_some() {
            let [yaw, pitch] = self.record_rot(c.record);
            if s.x_rot.as_ref().is_some_and(|r| !rot_test(r, pitch)) || s.y_rot.as_ref().is_some_and(|r| !rot_test(r, yaw)) {
                return false;
            }
        }
        if !s.scores.is_empty() {
            // The holder's name formatted once per entity, not once per objective.
            let holder: Cow<'_, str> = match c.record {
                Record::Player => Cow::Borrowed(&self.name),
                r => Cow::Owned(nbtio::uuid_text(self.record_uuid(r))),
            };
            for (obj, r) in &s.scores {
                match self.cmd.scores.get(&holder, obj) {
                    Some(v) if r.test(f64::from(v)) => {}
                    _ => return false,
                }
            }
        }
        true
    }

    /// Option `name=`: the name flattened in the server's language, a type's translated default
    /// name made once per search.
    fn record_passes_name<'a>(&'a self, s: &Sel, prep: &mut Prepared<'a>, c: &Candidate<'a>) -> bool {
        if s.names.is_empty() {
            return true;
        }
        let name = self.record_name(prep, c);
        s.names.iter().all(|(neg, n)| (name == *n) != *neg)
    }

    /// Advancements, NBT and data pack predicates: rare and costly, read through the identifier.
    fn record_passes_costly(&self, s: &Sel, e: E) -> bool {
        if !self.selector_advancements(&s.advancements, e) {
            return false;
        }
        if !s.nbts.is_empty() {
            let n = self.w_nbt(e.into());
            if s.nbts.iter().any(|(neg, x)| nbt_matches(x, &n) == *neg) {
                return false;
            }
        }
        s.predicates.iter().all(|(inverted, id)| self.selector_predicate(id, *inverted, e))
    }

    // ---- Record readers: the same values as the readers by identifier in `ents.rs`. ----

    fn record_type<'a>(&self, r: Record<'a>) -> &'a str {
        match r {
            Record::Player => "minecraft:player",
            Record::Mob(m) => m.kind.info().name,
            Record::Item(_) => "minecraft:item",
            Record::Orb(_) => "minecraft:experience_orb",
            Record::Kept(n) => kept_type(n),
            Record::Cart(c) => c.kind.name(),
        }
    }

    pub(super) fn record_pos(&self, r: Record<'_>) -> [f64; 3] {
        match r {
            Record::Player => self.pos,
            Record::Mob(m) => m.pos(),
            Record::Item(d) => d.pos,
            Record::Orb(o) => o.pos,
            Record::Kept(n) => nbtio::f64s::<3>(n.get("Pos")).unwrap_or_default(),
            Record::Cart(c) => c.pos,
        }
    }

    /// Yaw, pitch.
    pub(super) fn record_rot(&self, r: Record<'_>) -> [f32; 2] {
        match r {
            Record::Player => [self.yaw, self.pitch],
            Record::Mob(m) => [m.yaw, m.pitch],
            Record::Item(_) | Record::Orb(_) => [0.0; 2],
            Record::Kept(n) => nbtio::f32s::<2>(n.get("Rotation")).unwrap_or_default(),
            Record::Cart(c) => [c.yaw, c.pitch],
        }
    }

    pub(super) fn record_uuid(&self, r: Record<'_>) -> u128 {
        match r {
            Record::Player => self.uuid,
            Record::Mob(m) => m.uuid,
            Record::Item(d) => crate::drops::ground_uuid(d.id),
            Record::Orb(o) => crate::drops::ground_uuid(o.id),
            Record::Kept(n) => nbtio::uuid(n.get("UUID")).unwrap_or(0),
            Record::Cart(c) => c.uuid,
        }
    }

    /// The kept fields of a kept compound or a minecart (what `Server::other` returns).
    fn record_compound<'a>(&self, r: Record<'a>) -> Option<&'a Nbt> {
        match r {
            Record::Kept(n) => Some(n),
            Record::Cart(c) => Some(&c.nbt),
            _ => None,
        }
    }

    fn record_tags<'a>(&'a self, r: Record<'a>) -> Option<&'a Nbt> {
        match r {
            Record::Player => self.pdata.root.get("Tags"),
            Record::Mob(m) => m.nbt.as_ref()?.get("Tags"),
            Record::Item(_) | Record::Orb(_) => None,
            Record::Kept(_) | Record::Cart(_) => self.record_compound(r)?.get("Tags"),
        }
    }

    fn record_dims(&self, r: Record<'_>) -> [f32; 3] {
        use crate::decoration::DKind;
        match r {
            Record::Player => {
                let (w, h, eye) = self.player_dims();
                [w as f32, h as f32, eye as f32]
            }
            Record::Mob(m) => {
                let (w, h, eye) = crate::mobs::species::dims(m.kind, m.is_baby());
                [w, h, eye]
            }
            _ => {
                if let Some(n) = self.record_compound(r)
                    && let Some(k @ (DKind::Stand | DKind::Mannequin)) = crate::decoration::kind_of(n)
                {
                    let (w, h, eye) = self.living_size(n, k == DKind::Stand);
                    return [w as f32, h as f32, eye as f32];
                }
                self.type_dims(self.record_type(r)).unwrap_or([0.25, 0.25, 0.2])
            }
        }
    }

    fn record_bbox(&self, r: Record<'_>) -> rc_physics::Aabb {
        // Painting and frames: box according to direction and size; small stand or marker.
        if let Some(b) = self.record_compound(r).and_then(|n| self.deco_box(n)) {
            return b;
        }
        let p = self.record_pos(r);
        let [w, h, _] = self.record_dims(r);
        let hw = f64::from(w) / 2.0;
        rc_physics::Aabb { min: [p[0] - hw, p[1], p[2] - hw], max: [p[0] + hw, p[1] + f64::from(h), p[2] + hw] }
    }

    /// Simple name flattened: player, custom name, carried item, or translated type.
    fn record_name<'a>(&'a self, prep: &mut Prepared<'a>, c: &Candidate<'a>) -> String {
        use super::cmds_ents::plain;
        let custom = match c.record {
            Record::Player => return self.name.clone(),
            Record::Mob(m) => m.custom_name.as_ref(),
            Record::Kept(_) | Record::Cart(_) => self.record_compound(c.record).and_then(|n| n.get("CustomName")),
            Record::Item(_) | Record::Orb(_) => None,
        };
        if let Some(name) = custom.and_then(rc_text::from_nbt) {
            return plain(&name);
        }
        if let Record::Item(d) = c.record {
            return plain(&self.stack_name(&d.stack));
        }
        let ty = self.record_type(c.record);
        if let Some((_, name)) = prep.default_names.iter().find(|(t, _)| *t == ty) {
            return name.clone();
        }
        let name = plain(&rc_text::Component::translate(format!("entity.{}", ty.replace(':', ".")), Vec::new()));
        prep.default_names.push((ty, name.clone()));
        name
    }

    // ---- Tests: every search checked against the former one. ----

    /// The former search gives the same list (not with a random order: it would draw again).
    #[cfg(test)]
    fn check_against_reference(&mut self, s: &Sel, src: &Src, found: &[E]) {
        if crate::mobs::index::verify() && s.sort != Sort::Random {
            let reference = self.select_reference(s, src);
            assert_eq!(found, reference.as_slice(), "selector candidate view differs: {s:?}");
        }
    }

    /// Random order: the matches before the shuffle are the former ones.
    #[cfg(test)]
    fn check_matches_against_reference(&self, s: &Sel, src: &Src, base: [f64; 3], player_searched: bool, found: &[E]) {
        if crate::mobs::index::verify() {
            let reference = self.reference_matches(s, src, base, player_searched);
            assert_eq!(found, reference.as_slice(), "selector candidate view differs before the shuffle: {s:?}");
        }
    }

    /// One world of a search of every world: its matches are the former ones, cut at the same
    /// budget.
    #[cfg(test)]
    fn check_world_matches_against_reference(
        &self,
        s: &Sel,
        src: &Src,
        base: [f64; 3],
        budget: usize,
        player_searched: bool,
        found: &[(E, f64)],
    ) {
        if crate::mobs::index::verify() {
            let mut reference = self.reference_matches(s, src, base, player_searched);
            reference.truncate(budget);
            let listed: Vec<E> = found.iter().map(|(e, _)| *e).collect();
            assert_eq!(listed, reference, "selector candidate view differs in a search of every world: {s:?}");
        }
    }
}

/// How many matches a search collects before its sort: an arbitrary order stops at the limit,
/// as the game collects until it; a caller that only asks for one match stops at the first,
/// except with a random order, whose shuffle draws on the world's random once per match.
pub(super) fn match_budget(s: &Sel, first_only: bool) -> usize {
    if first_only && s.sort != Sort::Random {
        return 1;
    }
    if s.sort == Sort::Arbitrary { s.limit } else { usize::MAX }
}

/// Record of entity `e` in one world's lists, the player excepted: the active world's or a parked
/// one's, the same test for both ([`WorldEntityLists::holds`] is whether it finds one).
pub(super) fn record_in<'a>(lists: &WorldEntityLists<'a>, e: E) -> Option<Record<'a>> {
    let (mobs, kept) = (lists.mobs, lists.kept);
    Some(match e {
        E::Player => return None,
        E::Mob(i) => Record::Mob(mobs.get(i).filter(|m| m.alive())?),
        E::Item(i) => Record::Item(lists.drops.iter().find(|d| d.id == i)?),
        E::Orb(i) => Record::Orb(lists.orbs.iter().find(|o| o.id == i)?),
        // The order of `Server::other`: chunk lists, End family, minecarts, the exploding entity.
        E::Other(i) => match lists.kept_index.find(kept, i) {
            Some((c, k)) => Record::Kept(&kept[&c][k].1),
            None => {
                let end_family = mobs.end.snapshots.iter().find(|(x, _)| *x == i);
                let cart = lists.carts.list.iter().flatten().find(|c| c.id == i && !c.removed);
                match (end_family, cart) {
                    (Some((_, n)), _) => Record::Kept(n),
                    (None, Some(c)) => Record::Cart(c),
                    (None, None) => Record::Kept(lists.exploding.filter(|(x, _)| *x == i).map(|(_, n)| n)?),
                }
            }
        },
    })
}

/// Type of a kept compound (empty when it has none, like the reader by identifier).
fn kept_type(n: &Nbt) -> &str {
    n.get("id").and_then(Nbt::as_str).unwrap_or_default()
}
