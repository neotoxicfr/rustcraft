//! Optimisation switches of the mob goals, each read once (`RUSTCRAFT_OPT_*`, on by default, `=0`
//! brings the old path back). The proof tests force a whole set per thread, so that two runs of
//! the same scenario on two threads compare the old and the new paths inside one test binary.

/// Every goal switch at once (tests force a whole set on their thread).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GoalSwitches {
    /// `RUSTCRAFT_OPT_MOB_SCAN_ONE_PASS` (shared with the animal scans): the nearest-mob search and
    /// the call for help test each mob as they meet it, instead of collecting candidates first and
    /// looking each one up again by identifier.
    pub one_pass_scans: bool,
    /// `RUSTCRAFT_OPT_RIDING_SEAT_DIRECT`: the seat of a mounted target is read from the vehicle
    /// the target itself names, instead of walking the passengers of every mob.
    pub riding_seat_direct: bool,
    /// `RUSTCRAFT_OPT_MELEE_EARLY_EXIT`: a target out of melee reach is refused before its seat is
    /// looked up.
    pub melee_early_exit: bool,
    /// `RUSTCRAFT_OPT_GOAL_FLAGS_CACHE`: the control flags of a goal and its "ticks every tick"
    /// mark are kept beside the goal when it is added, instead of being computed at each read.
    pub goal_flags_cache: bool,
    /// `RUSTCRAFT_OPT_TEMPT_FOOD_MEMO`: "does the player hold this food?" is answered once per
    /// tick and per food for the two held stacks, for all the tempted animals.
    pub tempt_food_memo: bool,
    /// `RUSTCRAFT_OPT_BLOCKIDS` (shared with the rest of the server): blocks compared by
    /// identifier (grass, water, turtle egg, powder snow, bedrock) instead of by name.
    pub block_ids: bool,
    /// `RUSTCRAFT_OPT_BLOCK_GOAL_BOX_PRECHECK`: a "go to a block" search walks nothing when no
    /// section of its box can hold a block the goal accepts.
    pub block_goal_box_precheck: bool,
}

#[cfg(test)]
impl GoalSwitches {
    /// The code before this work.
    pub(crate) const OLD: GoalSwitches = GoalSwitches {
        one_pass_scans: false,
        riding_seat_direct: false,
        melee_early_exit: false,
        goal_flags_cache: false,
        tempt_food_memo: false,
        block_ids: false,
        block_goal_box_precheck: false,
    };
    /// Every new path.
    pub(crate) const ALL_NEW: GoalSwitches = GoalSwitches {
        one_pass_scans: true,
        riding_seat_direct: true,
        melee_early_exit: true,
        goal_flags_cache: true,
        tempt_food_memo: true,
        block_ids: true,
        block_goal_box_precheck: true,
    };
}

#[cfg(test)]
thread_local! {
    /// Proof tests: the switches forced on this thread (`None`: the environment).
    pub(crate) static FORCED: std::cell::Cell<Option<GoalSwitches>> = const { std::cell::Cell::new(None) };
}

/// The forced set of this thread, if a test chose one.
#[cfg(test)]
fn forced() -> Option<GoalSwitches> {
    FORCED.get()
}

#[cfg(not(test))]
fn forced() -> Option<GoalSwitches> {
    None
}

pub(crate) fn one_pass_scans() -> bool {
    forced().map_or_else(crate::mobs::index::one_pass_scans, |f| f.one_pass_scans)
}

pub(crate) fn riding_seat_direct() -> bool {
    forced().map_or_else(|| rc_data::opt_switch!("RUSTCRAFT_OPT_RIDING_SEAT_DIRECT"), |f| f.riding_seat_direct)
}

pub(crate) fn melee_early_exit() -> bool {
    forced().map_or_else(|| rc_data::opt_switch!("RUSTCRAFT_OPT_MELEE_EARLY_EXIT"), |f| f.melee_early_exit)
}

pub(crate) fn goal_flags_cache() -> bool {
    forced().map_or_else(|| rc_data::opt_switch!("RUSTCRAFT_OPT_GOAL_FLAGS_CACHE"), |f| f.goal_flags_cache)
}

pub(crate) fn tempt_food_memo() -> bool {
    forced().map_or_else(|| rc_data::opt_switch!("RUSTCRAFT_OPT_TEMPT_FOOD_MEMO"), |f| f.tempt_food_memo)
}

pub(crate) fn block_ids() -> bool {
    forced().map_or_else(crate::mobs::env::block_ids_on, |f| f.block_ids)
}

pub(crate) fn block_goal_box_precheck() -> bool {
    forced()
        .map_or_else(|| rc_data::opt_switch!("RUSTCRAFT_OPT_BLOCK_GOAL_BOX_PRECHECK"), |f| f.block_goal_box_precheck)
}
