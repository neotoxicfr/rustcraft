//! Which optimised advancement paths a player's progress takes, each behind its switch (`=0`
//! restores the former way), read once per process when the progress is created.

/// Optimised paths of one player's advancement progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgressPaths {
    /// `RUSTCRAFT_OPT_ADVINDEX`: a changed inventory slot tests only the `inventory_changed`
    /// criteria that name its item or one of its tags, plus those no single item narrows
    /// (instead of the whole list, about 1,900 criteria in 26.3).
    pub item_index: bool,
    /// `RUSTCRAFT_OPT_RECIPEINDEX`: a learned recipe, a crafted recipe or a generated chest loot
    /// tests only the criteria naming that recipe or loot table (instead of every criterion of
    /// the trigger for each recipe: about 1,700 × 1,400 tests for `/recipe give @s *`).
    pub recipe_index: bool,
    /// `RUSTCRAFT_OPT_ADVLOADFAST`: the advancements file is read in one pass that keeps the
    /// order of its keys (instead of searching each key in the text), the visibility walk lends
    /// the children instead of copying them, and a started advancement is found by a flag.
    pub fast_load: bool,
    /// `RUSTCRAFT_OPT_ADVTREEPARALLEL`, OFF by default (`=1` turns it on): the ~1,900 advancement
    /// files of the tree parsed on every core. Slower than in order on the loaded-machine bench
    /// with every core (the tree is parsed while the generator fills the same thread pool).
    pub parallel_tree: bool,
    /// `RUSTCRAFT_OPT_LOCATIONFAST`: location predicates read the biome name by reference and
    /// look a block tag up by the block's index (instead of copying the name and scanning lists).
    pub location_fast: bool,
}

#[cfg(test)]
thread_local! {
    /// Tests: every indexed event also scans the trigger's whole list and requires the same hits.
    pub(crate) static CHECK_AGAINST_FULL_SCAN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Tests and benches: paths of the progress (and trees) created by this thread.
    static FORCED_PATHS: std::cell::Cell<Option<ProgressPaths>> = const { std::cell::Cell::new(None) };
}

impl ProgressPaths {
    pub fn from_switches() -> ProgressPaths {
        #[cfg(test)]
        if let Some(paths) = FORCED_PATHS.with(std::cell::Cell::get) {
            return paths;
        }
        ProgressPaths {
            item_index: rc_data::opt_switch!("RUSTCRAFT_OPT_ADVINDEX"),
            recipe_index: rc_data::opt_switch!("RUSTCRAFT_OPT_RECIPEINDEX"),
            fast_load: rc_data::opt_switch!("RUSTCRAFT_OPT_ADVLOADFAST"),
            parallel_tree: rc_data::opt_switch!("RUSTCRAFT_OPT_ADVTREEPARALLEL", default_off),
            location_fast: rc_data::opt_switch!("RUSTCRAFT_OPT_LOCATIONFAST"),
        }
    }

    /// Every path off: the code before the optimisation.
    #[cfg(test)]
    pub const OLD: ProgressPaths = ProgressPaths {
        item_index: false,
        recipe_index: false,
        fast_load: false,
        parallel_tree: false,
        location_fast: false,
    };

    /// Every path on: the defaults, plus the tree parsed on every core (off by default).
    #[cfg(test)]
    pub const NEW: ProgressPaths = ProgressPaths {
        item_index: true,
        recipe_index: true,
        fast_load: true,
        parallel_tree: true,
        location_fast: true,
    };

    /// Runs `f` with the progress it creates on `paths`.
    #[cfg(test)]
    pub(crate) fn forced<T>(paths: ProgressPaths, f: impl FnOnce() -> T) -> T {
        let before = FORCED_PATHS.with(|p| p.replace(Some(paths)));
        let out = f();
        FORCED_PATHS.with(|p| p.set(before));
        out
    }

    /// Runs `f` with every indexed event checked against the full scan.
    #[cfg(test)]
    pub(crate) fn checked<T>(f: impl FnOnce() -> T) -> T {
        let before = CHECK_AGAINST_FULL_SCAN.with(|c| c.replace(true));
        let out = f();
        CHECK_AGAINST_FULL_SCAN.with(|c| c.set(before));
        out
    }
}

/// Whether indexed events are checked against the full scan (tests only).
#[inline]
pub(crate) fn check_against_full_scan() -> bool {
    #[cfg(test)]
    return CHECK_AGAINST_FULL_SCAN.with(std::cell::Cell::get);
    #[cfg(not(test))]
    false
}
