//! Full light of the chunks of a batch: each chunk lit alone over its 3 × 3 columns, in parallel.

use std::sync::atomic::Ordering::Relaxed;

use rayon::prelude::*;

use super::{ColumnScan, Columns, FullOpts, LightSheets, Scans};
use crate::{LightTables, stats};

/// Light sheets of each chunk of `chunks`, in that order; `column` and `scan` give the states and
/// the scan (if done) of a column. Counted in `stats` like [`crate::light_chunk`].
pub(crate) fn light_chunks<'a, C, S>(
    t: &LightTables,
    chunks: &[(i32, i32)],
    column: C,
    scan: S,
    height: i32,
    has_sky: bool,
    opts: FullOpts,
) -> Vec<LightSheets>
where
    C: Fn(i32, i32) -> Option<&'a [u32]> + Sync,
    S: Fn(i32, i32) -> Option<&'a ColumnScan> + Sync,
{
    let alone = |&(x, z): &(i32, i32)| {
        let cols: Columns = std::array::from_fn(|i| column(x + i as i32 % 3 - 1, z + i as i32 / 3 - 1));
        let scans: Scans = std::array::from_fn(|i| scan(x + i as i32 % 3 - 1, z + i as i32 / 3 - 1));
        counted(|| crate::light_chunk_dispatch(t, cols, scans, height, has_sky, opts))
    };
    chunks.par_iter().map(alone).collect()
}

/// `f`'s lighting of one chunk counted in `stats`.
fn counted<R>(f: impl FnOnce() -> R) -> R {
    let t0 = std::time::Instant::now();
    let out = f();
    let ns = t0.elapsed().as_nanos() as u64;
    stats::CHUNK_NS.fetch_add(ns, Relaxed);
    stats::add_this_thread_light_ns(ns);
    stats::CHUNKS.fetch_add(1, Relaxed);
    out
}
