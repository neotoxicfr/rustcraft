//! Evaluation counts (tests and benches): how much a chunk evaluates at a point, how the caches
//! answer, what the choices restricted in y compute. Counted per thread, only while
//! [`count_evaluations`] is on; off (the game), each counting site costs one load of a static
//! and a branch that always goes the same way.

use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

/// Declares the counters once: the structure and the difference of two counts.
macro_rules! counters {
    ($($(#[$doc:meta])* $field:ident),* $(,)?) => {
        /// What the evaluator did on this thread while [`count_evaluations`] was on.
        #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
        pub struct EvalCounts {
            $($(#[$doc])* pub $field: u64,)*
        }

        /// Counts of one step: the counts after it minus the counts before.
        impl std::ops::Sub for EvalCounts {
            type Output = EvalCounts;
            fn sub(self, before: EvalCounts) -> EvalCounts {
                EvalCounts { $($field: self.$field - before.$field,)* }
            }
        }
    };
}

counters!(
    /// Noises evaluated at a point (plain, shifted, shift B, end islands).
    point_noises,
    /// Splines evaluated at a point.
    point_splines,
    /// Densities probed by a top surface search.
    top_probes,
    /// Point reads of a cache answered by its last point.
    cache_point_last,
    /// Point reads of a cache answered by its volume buffer.
    cache_point_buffer,
    /// Point reads of a cache answered from another height of the column (input without y).
    cache_point_same_column,
    /// Point reads of a cache that computed their value.
    cache_point_computed,
    /// Volume reads of a cache answered by the same volume.
    cache_volume_same,
    /// Volume reads of a cache answered by rows of the cached volume.
    cache_volume_rows,
    /// Volume reads of a cache answered by a sub-lattice of the cached volume.
    cache_volume_sublattice,
    /// Volume reads of a cache that computed their volume.
    cache_volume_computed,
    /// Values computed by the volume reads of a cache that missed.
    cache_volume_values,
    /// Branches of choices restricted in y, computed over a volume.
    choice_branches,
    /// Elements of those volumes that read the branch.
    branch_values_read,
    /// Values computed for those branches: rows of the hull of every column, times the columns.
    branch_values_whole_hull,
    /// Values a hull per column would compute instead.
    branch_values_column_hulls,
    /// Noise values the branches without interpolation compute over the hull of every column:
    /// values times the noise nodes of the branch.
    plain_branch_noise_values_whole_hull,
    /// Noise values a hull per column would compute instead.
    plain_branch_noise_values_column_hulls,
    /// Volumes of a shifted noise (in x and z, or in x, y and z).
    shifted_noise_volumes,
    /// Values of those volumes (points handed to the noise).
    shifted_noise_values,
    /// Those of the volumes that hold more than one row in y.
    shifted_noise_volumes_taller_than_one_row,
);

/// Callers that turned the counts on and have not turned them off yet (tests run in parallel).
static COUNTING: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static COUNTS: Cell<EvalCounts> = Cell::new(EvalCounts::default());
}

/// Turns the evaluation counts on for every thread, or gives back one earlier
/// `count_evaluations(true)`: they stay on until each caller that turned them on is done.
#[doc(hidden)]
pub fn count_evaluations(on: bool) {
    if on {
        COUNTING.fetch_add(1, Relaxed);
    } else {
        let _ = COUNTING.try_update(Relaxed, Relaxed, |n| n.checked_sub(1));
    }
}

/// Counts gathered by this thread since the last call, then zeroed.
#[doc(hidden)]
pub fn take_eval_counts() -> EvalCounts {
    COUNTS.with(Cell::take)
}

/// Adds to this thread's counts when counting is on.
#[inline]
pub(super) fn count(add: impl FnOnce(&mut EvalCounts)) {
    if COUNTING.load(Relaxed) != 0 {
        bump(add);
    }
}

#[cold]
fn bump(add: impl FnOnce(&mut EvalCounts)) {
    COUNTS.with(|c| {
        let mut n = c.get();
        add(&mut n);
        c.set(n);
    });
}
