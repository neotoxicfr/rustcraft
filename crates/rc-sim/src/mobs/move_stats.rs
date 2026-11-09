//! Rest rate of the mobs' movements, a measurement (`RUSTCRAFT_PERFLOG`): the share of movements
//! whose inputs (box, movement, ground flag, step height, powder snow and lava envelopes) are those
//! of the same mob's movement in the previous tick while no block of the columns they cover was
//! written since. It is the hit rate a memo of the movement could reach (it would skip the
//! collision search). Also the share of support searches answered from the boxes the movement kept
//! (`RUSTCRAFT_OPT_SUPPORTSHARE`). Without the variable: one cached boolean per movement.
//!
//! Writes are seen through the server's block write; a chunk loaded or unloaded next to a mob is
//! not counted as a change (the crowd scenes load nothing near their mobs).

use std::cell::RefCell;

use hashbrown::HashMap;

/// Counting active (`RUSTCRAFT_PERFLOG` present at startup).
pub(crate) fn enabled() -> bool {
    crate::mem_stats::enabled()
}

/// Inputs of a movement that decide its result, compared bit for bit.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct MoveInputs {
    box_bits: [u64; 6],
    delta_bits: [u64; 3],
    on_ground: bool,
    step_bits: u64,
    snow_envelope: bool,
    lava_envelope: bool,
}

impl MoveInputs {
    pub(crate) fn new(start: &rc_physics::Aabb, delta: [f64; 3], on_ground: bool, step: f64, snow: bool, lava: bool) -> Self {
        let b = [start.min[0], start.min[1], start.min[2], start.max[0], start.max[1], start.max[2]];
        MoveInputs {
            box_bits: b.map(f64::to_bits),
            delta_bits: delta.map(f64::to_bits),
            on_ground,
            step_bits: step.to_bits(),
            snow_envelope: snow,
            lava_envelope: lava,
        }
    }
}

#[derive(Default)]
struct MoveStats {
    /// Last movement of each mob: inputs and server tick.
    last: HashMap<i32, (MoveInputs, u64)>,
    /// Columns written, with the server tick of their last write.
    written: HashMap<(i32, i32), u64>,
    moves: u64,
    at_rest: u64,
    supports: u64,
    supports_shared: u64,
}

thread_local! {
    static STATS: RefCell<MoveStats> = RefCell::new(MoveStats::default());
}

/// A block of column `p` written at server tick `tick`.
pub(crate) fn column_written(p: [i32; 3], tick: u64) {
    STATS.with(|s| {
        s.borrow_mut().written.insert((p[0] >> 4, p[2] >> 4), tick);
    });
}

/// Movement of mob `id` at server tick `tick`; `zone`: what its collision search may read (the box
/// stretched by the movement and the step height).
pub(crate) fn movement(id: i32, tick: u64, inputs: MoveInputs, zone: &rc_physics::Aabb) {
    STATS.with(|s| {
        let s = &mut *s.borrow_mut();
        s.moves += 1;
        let same = s.last.get(&id).is_some_and(|&(before, at)| before == inputs && at + 1 == tick);
        if same && !written_since(&s.written, zone, tick - 1) {
            s.at_rest += 1;
        }
        s.last.insert(id, (inputs, tick));
    });
}

/// A block of the columns the walk of `zone` reads (one cell of margin) written at `since` or later.
fn written_since(written: &HashMap<(i32, i32), u64>, zone: &rc_physics::Aabb, since: u64) -> bool {
    let column = |v: f64| (v.floor() as i32) >> 4;
    let (x0, x1) = (column(zone.min[0] - 1.0), column(zone.max[0] + 1.0));
    let (z0, z1) = (column(zone.min[2] - 1.0), column(zone.max[2] + 1.0));
    (x0..=x1).any(|cx| (z0..=z1).any(|cz| written.get(&(cx, cz)).is_some_and(|&t| t >= since)))
}

/// A support search after a movement; `shared`: answered from the kept boxes.
pub(crate) fn support_search(shared: bool) {
    STATS.with(|s| {
        let s = &mut *s.borrow_mut();
        s.supports += 1;
        s.supports_shared += u64::from(shared);
    });
}

/// Report line of the counts since the last one, then the counts reset; `None` without movement.
pub(crate) fn take_report() -> Option<String> {
    STATS.with(|s| {
        let s = &mut *s.borrow_mut();
        if s.moves == 0 {
            return None;
        }
        let pct = |n: u64, of: u64| if of == 0 { 0.0 } else { n as f64 * 100.0 / of as f64 };
        let line = format!(
            "[perf] mob movements: {} at rest {:.1} %, support searches {} from the kept boxes {:.1} %",
            s.moves,
            pct(s.at_rest, s.moves),
            s.supports,
            pct(s.supports_shared, s.supports)
        );
        (s.moves, s.at_rest, s.supports, s.supports_shared) = (0, 0, 0, 0);
        Some(line)
    })
}

/// Every 100 server ticks, the report line on the error output; the movements and writes older
/// than the last tick are dropped (gone mobs, columns written long ago).
pub(crate) fn report_every_100_ticks(tick: u64) {
    if !enabled() || !tick.is_multiple_of(100) {
        return;
    }
    STATS.with(|s| {
        let s = &mut *s.borrow_mut();
        s.last.retain(|_, (_, at)| *at + 1 >= tick);
        s.written.retain(|_, at| *at + 1 >= tick);
    });
    if let Some(line) = take_report() {
        eprintln!("{line}");
    }
}
