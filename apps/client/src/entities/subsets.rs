//! Rare subsets of the entities kept as they change, so that a frame or a tick does not walk every
//! entity to find the few it works on (glowing outlines, ghosts, name tags, guardian beams,
//! fishing lines, monster effects, mob effect swirls).

use hashbrown::{HashMap, HashSet};

use super::Entity;

/// `RUSTCRAFT_OPT_ENTITY_SUBSETS=0`: glowing, invisible, name-tag candidates and carriers of effect
/// particles found by walking every entity on every frame or tick (old path) instead of the sets
/// kept by `Entities::apply`.
pub(crate) fn entity_subsets_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_ENTITY_SUBSETS")
}

/// `RUSTCRAFT_OPT_RARE_KIND_LISTS=0`: guardians, fishing bobbers and monsters with client effects
/// found by walking every entity (old path) instead of the lists kept at spawn.
pub(crate) fn rare_kind_lists_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_RARE_KIND_LISTS")
}

/// Ids of the entities that may belong to a rare subset. A set may still hold an id that left its
/// subset or the world (flag cleared, kind replaced by a new spawn, entity removed elsewhere than
/// by `Entities::apply`): every reader checks the entity again ([`EntitySubsets::is_glowing`]
/// and the like), and each tick drops those ids. What matters is that no member is ever missing.
#[derive(Default)]
pub struct EntitySubsets {
    /// Data 0 received with the glowing flag (0x40).
    pub glowing: HashSet<i32>,
    /// Data 0 received with the invisible flag (0x20).
    pub invisible: HashSet<i32>,
    /// Players, and data 3 (custom name always visible) received true: with the aimed entity, the
    /// only ones that can show a name tag.
    pub tag_candidates: HashSet<i32>,
    /// Guardians and elder guardians (eyes and beams).
    pub guardians: HashSet<i32>,
    /// Fishing bobbers (owners and lines).
    pub bobbers: HashSet<i32>,
    /// Monsters with client effects (`entity_fx::has_monster_effects`).
    pub effect_mobs: HashSet<i32>,
    /// Firework rockets (those attached to the local player boost its glide).
    pub rockets: HashSet<i32>,
    /// Living entities whose data 10 lists at least one effect particle (swirls of `entity_fx`).
    pub effect_particles: HashSet<i32>,
}

impl EntitySubsets {
    pub fn is_glowing(e: &Entity) -> bool {
        e.flags() & 0x40 != 0
    }

    pub fn is_invisible(e: &Entity) -> bool {
        e.flags() & 0x20 != 0
    }

    pub fn is_tag_candidate(e: &Entity) -> bool {
        e.kind == "player" || e.meta_bool(3)
    }

    pub fn is_guardian(e: &Entity) -> bool {
        matches!(e.kind.as_str(), "guardian" | "elder_guardian")
    }

    pub fn is_bobber(e: &Entity) -> bool {
        e.kind == "fishing_bobber"
    }

    pub fn is_effect_mob(e: &Entity) -> bool {
        crate::entity_fx::has_monster_effects(&e.kind)
    }

    pub fn is_rocket(e: &Entity) -> bool {
        e.kind == "firework_rocket"
    }

    pub fn has_effect_particles(e: &Entity) -> bool {
        !crate::entity_fx::effect_particle_list(e).is_empty()
    }

    pub fn clear(&mut self) {
        *self = EntitySubsets::default();
    }

    /// Entity `id` just spawned (no data yet): its kind decides the lists.
    pub fn note_spawn(&mut self, id: i32, e: &Entity) {
        if entity_subsets_on() && Self::is_tag_candidate(e) {
            self.tag_candidates.insert(id);
        }
        if !rare_kind_lists_on() {
            return;
        }
        if Self::is_guardian(e) {
            self.guardians.insert(id);
        } else if Self::is_bobber(e) {
            self.bobbers.insert(id);
        } else if Self::is_effect_mob(e) {
            self.effect_mobs.insert(id);
        } else if Self::is_rocket(e) {
            self.rockets.insert(id);
        }
    }

    /// Data 0, 3 or 10 of entity `id` just changed: it joins the sets whose rule it now meets (it
    /// leaves the others at the next tick).
    pub fn note_data(&mut self, id: i32, e: &Entity) {
        if !entity_subsets_on() {
            return;
        }
        if Self::is_glowing(e) {
            self.glowing.insert(id);
        }
        if Self::is_invisible(e) {
            self.invisible.insert(id);
        }
        if Self::is_tag_candidate(e) {
            self.tag_candidates.insert(id);
        }
        if Self::has_effect_particles(e) {
            self.effect_particles.insert(id);
        }
    }

    /// Whether a data index decides one of the sets of [`EntitySubsets::note_data`].
    pub fn is_subset_data(index: u8) -> bool {
        matches!(index, 0 | 3 | crate::entity_fx::EFFECT_PARTICLE_LIST)
    }

    /// Once per tick: ids whose entity is gone or no longer meets the rule leave their set.
    pub fn drop_stale(&mut self, map: &HashMap<i32, Entity>) {
        let keep = |set: &mut HashSet<i32>, rule: fn(&Entity) -> bool| set.retain(|id| map.get(id).is_some_and(rule));
        keep(&mut self.glowing, Self::is_glowing);
        keep(&mut self.invisible, Self::is_invisible);
        keep(&mut self.tag_candidates, Self::is_tag_candidate);
        keep(&mut self.guardians, Self::is_guardian);
        keep(&mut self.bobbers, Self::is_bobber);
        keep(&mut self.effect_mobs, Self::is_effect_mob);
        keep(&mut self.rockets, Self::is_rocket);
        keep(&mut self.effect_particles, Self::has_effect_particles);
    }
}

impl super::Entities {
    /// Entities of a rare kind (`rule`): those of the list `kept` filled at spawn, or every entity
    /// walked (`RUSTCRAFT_OPT_RARE_KIND_LISTS=0`). The order is that of the list or of the table:
    /// readers do not depend on it (sets, opaque quads), or only for client-side randomness.
    pub fn of_rare_kind<'a>(
        &'a self,
        kept: &'a HashSet<i32>,
        rule: fn(&Entity) -> bool,
    ) -> impl Iterator<Item = &'a Entity> + 'a {
        self.kept_or_walked(rare_kind_lists_on(), kept, rule)
    }

    /// Living entities carrying effect particles: those of the kept set, or every entity walked
    /// (`RUSTCRAFT_OPT_ENTITY_SUBSETS=0`). Order of the set or of the table: only client-side
    /// randomness depends on it.
    pub fn with_effect_particles(&self) -> impl Iterator<Item = &Entity> + '_ {
        let kept = &self.subsets.effect_particles;
        self.kept_or_walked(entity_subsets_on(), kept, EntitySubsets::has_effect_particles)
    }

    /// Entities meeting `rule`, found through the ids of `kept` when `from_kept`, otherwise by
    /// walking the table.
    fn kept_or_walked<'a>(
        &'a self,
        from_kept: bool,
        kept: &'a HashSet<i32>,
        rule: fn(&Entity) -> bool,
    ) -> impl Iterator<Item = &'a Entity> + 'a {
        let from_list = from_kept.then(|| kept.iter().filter_map(|id| self.map.get(id))).into_iter().flatten();
        let from_walk = (!from_kept).then(|| self.map.values()).into_iter().flatten();
        from_list.chain(from_walk).filter(move |e| rule(e))
    }
}
