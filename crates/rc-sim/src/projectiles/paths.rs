//! Switches of the projectile optimisations, each read once: `RUSTCRAFT_OPT_<NAME>=0` restores the
//! former path. A test forces a set of paths on its own thread (`FORCED_PROJECTILE_PATHS`), so that
//! the equivalence proofs run every path in one process.

use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProjectilePaths {
    /// `RUSTCRAFT_OPT_PROJECTILE_TAKE_HEAVY_FIELDS`: a projectile's stacks, entity data and pierced
    /// lists move out of its slot for the time of its tick (formerly the whole projectile was
    /// copied out of its slot every tick, then written back).
    pub take_heavy_fields: bool,
    /// `RUSTCRAFT_OPT_PROJECTILE_BORROWED_SHAPES`: the block clips of projectiles and the bobber's
    /// open-water test borrow each cell's collision shape (formerly copied into a new list per cell).
    pub borrowed_shapes: bool,
    /// `RUSTCRAFT_OPT_PROJECTILE_OWNER_INDEX`: the shooter is found through the mobs' UUID index
    /// (formerly a walk of the whole mob list at every lookup).
    pub owner_index: bool,
    /// `RUSTCRAFT_OPT_PROJECTILE_TARGET_PREFILTER`: a mob whose species bound cannot reach the
    /// search area is skipped before its box is worked out.
    pub target_prefilter: bool,
    /// `RUSTCRAFT_OPT_PROJECTILE_TARGETS_IN_PLACE`: the nearest target is picked while the targets
    /// are walked (formerly gathered into a new list first, every tick of every flying projectile).
    pub targets_in_place: bool,
    /// `RUSTCRAFT_OPT_HIT_WITHOUT_MOB_CLONE`: a mob's projectile hurting the player copies the mob
    /// once, as the damage source (formerly twice), and a mob's mace smash borrows the attacker
    /// instead of copying it.
    pub hit_without_mob_clone: bool,
    /// `RUSTCRAFT_OPT_CHORUS_TAG_BITS`: the chorus fruit reads its block tags through the state's
    /// block index (formerly the block's name searched in the whole block list, at every cell).
    pub chorus_tag_bits: bool,
    /// `RUSTCRAFT_OPT_FISHING_STATE_BITS`: the bobber's open-water and water tests read the
    /// state's flags, block index and fluid (formerly names and properties compared as text).
    pub fishing_state_bits: bool,
    /// Deliberate fault of the equivalence proofs (always `None` outside them).
    pub sabotage: Sabotage,
}

/// Deliberate faults, set only by the equivalence proofs to show that they see them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum Sabotage {
    None,
    /// After a dimension change during its tick, the projectile's slot keeps no stack nor data.
    ShellLeftInParkedList,
    /// The nearest target is the last one met at equal distance instead of the first.
    LastTargetAtEqualDistance,
}

#[cfg(test)]
thread_local! {
    /// Tests: paths of the projectiles of this thread (`None`: the switches).
    pub(crate) static FORCED_PROJECTILE_PATHS: std::cell::Cell<Option<ProjectilePaths>> =
        const { std::cell::Cell::new(None) };
}

impl ProjectilePaths {
    /// Every new path.
    pub(crate) const NEW: ProjectilePaths = ProjectilePaths {
        take_heavy_fields: true,
        borrowed_shapes: true,
        owner_index: true,
        target_prefilter: true,
        targets_in_place: true,
        hit_without_mob_clone: true,
        chorus_tag_bits: true,
        fishing_state_bits: true,
        sabotage: Sabotage::None,
    };

    /// Every former path.
    #[cfg(test)]
    pub(crate) const OLD: ProjectilePaths = ProjectilePaths {
        take_heavy_fields: false,
        borrowed_shapes: false,
        owner_index: false,
        target_prefilter: false,
        targets_in_place: false,
        hit_without_mob_clone: false,
        chorus_tag_bits: false,
        fishing_state_bits: false,
        sabotage: Sabotage::None,
    };

    pub(crate) fn current() -> ProjectilePaths {
        #[cfg(test)]
        if let Some(forced) = FORCED_PROJECTILE_PATHS.get() {
            return forced;
        }
        static PATHS: OnceLock<ProjectilePaths> = OnceLock::new();
        *PATHS.get_or_init(|| ProjectilePaths {
            take_heavy_fields: rc_data::opt_switch!("RUSTCRAFT_OPT_PROJECTILE_TAKE_HEAVY_FIELDS"),
            borrowed_shapes: rc_data::opt_switch!("RUSTCRAFT_OPT_PROJECTILE_BORROWED_SHAPES"),
            owner_index: rc_data::opt_switch!("RUSTCRAFT_OPT_PROJECTILE_OWNER_INDEX"),
            target_prefilter: rc_data::opt_switch!("RUSTCRAFT_OPT_PROJECTILE_TARGET_PREFILTER"),
            targets_in_place: rc_data::opt_switch!("RUSTCRAFT_OPT_PROJECTILE_TARGETS_IN_PLACE"),
            hit_without_mob_clone: rc_data::opt_switch!("RUSTCRAFT_OPT_HIT_WITHOUT_MOB_CLONE"),
            chorus_tag_bits: rc_data::opt_switch!("RUSTCRAFT_OPT_CHORUS_TAG_BITS"),
            fishing_state_bits: rc_data::opt_switch!("RUSTCRAFT_OPT_FISHING_STATE_BITS"),
            ..ProjectilePaths::NEW
        })
    }
}
