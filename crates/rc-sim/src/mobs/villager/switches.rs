//! Optimisation switches of the villager code, each read once (`RUSTCRAFT_OPT_*`, on by default,
//! `=0` brings the old path back). The proof tests force them per thread, so that two runs of the
//! same scenario on two threads compare the old and the new path inside one test binary.

/// Every villager switch at once (tests force a whole set on their thread).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VillagerSwitches {
    /// `RUSTCRAFT_OPT_VILLAGER_VISIBLE_BY_INDEX`: the search among the visible entities tests each
    /// candidate by its index in the list instead of copying the list and looking it up again.
    pub visible_by_index: bool,
    /// `RUSTCRAFT_OPT_VILLAGER_SENSED_KIND`: species read once by the sensor and compared as a
    /// species (no lookup of the mob, no comparison of names).
    pub sensed_kind: bool,
    /// `RUSTCRAFT_OPT_VILLAGER_NO_TICK_ALLOCATIONS`: entry tables, borrowed lists and reused
    /// buffers instead of the vectors built at each tick.
    pub no_tick_allocations: bool,
    /// `RUSTCRAFT_OPT_VILLAGER_MOVE_PATHS`: the computed path is moved into the navigation instead
    /// of being cloned there.
    pub move_paths: bool,
    /// `RUSTCRAFT_OPT_VILLAGER_FOOD_CACHE`: food value of the item's default component read
    /// from a per-item table built once.
    pub food_cache: bool,
    /// `RUSTCRAFT_OPT_TRADE_BORROW`: offers lent instead of copied (display, trading menu sync,
    /// price of a payment).
    pub trade_borrow: bool,
    /// `RUSTCRAFT_OPT_BLOCKIDS` (shared with the rest of the server): blocks compared by
    /// identifier (farmland, crops) instead of by name.
    pub block_ids: bool,
    /// `RUSTCRAFT_OPT_VILLAGER_FARMLAND_BOX_SCAN`: the farmer's farmland sensing scans its box section
    /// by section, skipping the sections without farmland, instead of reading the 405 cells one by one.
    pub farmland_box_scan: bool,
}

#[cfg(test)]
impl VillagerSwitches {
    /// The code before this work.
    pub(crate) const OLD: VillagerSwitches = VillagerSwitches {
        visible_by_index: false,
        sensed_kind: false,
        no_tick_allocations: false,
        move_paths: false,
        food_cache: false,
        trade_borrow: false,
        block_ids: false,
        farmland_box_scan: false,
    };
    /// Every new path.
    pub(crate) const ALL_NEW: VillagerSwitches = VillagerSwitches {
        visible_by_index: true,
        sensed_kind: true,
        no_tick_allocations: true,
        move_paths: true,
        food_cache: true,
        trade_borrow: true,
        block_ids: true,
        farmland_box_scan: true,
    };
}

#[cfg(test)]
thread_local! {
    /// Proof tests: the switches forced on this thread (`None`: the environment).
    pub(crate) static FORCED: std::cell::Cell<Option<VillagerSwitches>> = const { std::cell::Cell::new(None) };
}

/// The forced set of this thread, if a test chose one.
#[cfg(test)]
fn forced() -> Option<VillagerSwitches> {
    FORCED.get()
}

#[cfg(not(test))]
fn forced() -> Option<VillagerSwitches> {
    None
}

pub(crate) fn visible_by_index() -> bool {
    forced().map_or_else(|| rc_data::opt_switch!("RUSTCRAFT_OPT_VILLAGER_VISIBLE_BY_INDEX"), |f| f.visible_by_index)
}

pub(crate) fn sensed_kind() -> bool {
    forced().map_or_else(|| rc_data::opt_switch!("RUSTCRAFT_OPT_VILLAGER_SENSED_KIND"), |f| f.sensed_kind)
}

pub(crate) fn no_tick_allocations() -> bool {
    forced()
        .map_or_else(|| rc_data::opt_switch!("RUSTCRAFT_OPT_VILLAGER_NO_TICK_ALLOCATIONS"), |f| f.no_tick_allocations)
}

pub(crate) fn move_paths() -> bool {
    forced().map_or_else(|| rc_data::opt_switch!("RUSTCRAFT_OPT_VILLAGER_MOVE_PATHS"), |f| f.move_paths)
}

pub(crate) fn food_cache() -> bool {
    forced().map_or_else(|| rc_data::opt_switch!("RUSTCRAFT_OPT_VILLAGER_FOOD_CACHE"), |f| f.food_cache)
}

pub(crate) fn trade_borrow() -> bool {
    forced().map_or_else(|| rc_data::opt_switch!("RUSTCRAFT_OPT_TRADE_BORROW"), |f| f.trade_borrow)
}

pub(crate) fn block_ids() -> bool {
    forced().map_or_else(crate::mobs::env::block_ids_on, |f| f.block_ids)
}

pub(crate) fn farmland_box_scan() -> bool {
    forced().map_or_else(|| rc_data::opt_switch!("RUSTCRAFT_OPT_VILLAGER_FARMLAND_BOX_SCAN"), |f| f.farmland_box_scan)
}
