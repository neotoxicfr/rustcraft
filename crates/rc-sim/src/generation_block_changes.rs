//! Block changes of the server handed to the generation thread.
//!
//! The thread lights each new chunk, and relights the full chunks it already returned next to it,
//! from its own copy of their blocks. The game has a single light engine that always reads the live
//! blocks, so that copy must follow the server's changes (a torch placed at the edge of the explored
//! world, a dug hole, flowing water): they are sent to the thread, numbered, applied to the chunks it
//! holds and kept until a save writes them, so that a chunk coming back (set aside, or read back from
//! disk) catches up. Only the chunks the thread may still read are concerned: it reads a full chunk
//! only as a neighbour of a chunk it makes full or of one it relights, so never once the 25 chunks
//! within two of it are all full on the server.
//!
//! A change can still reach the thread after a batch computed its light. Before the outputs that
//! follow a newly applied message, the thread reports the last message they include (`Out::ChangesApplied`);
//! the server relights the changes they lack itself, after taking that light, as it relights the
//! changes of a tick.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use hashbrown::HashMap;

use super::{Out, Worker};

/// A block change: world position, new state.
pub(crate) type BlockChange = ([i32; 3], u32);

/// Server side of the changes sent to the generation thread.
#[derive(Default)]
pub(crate) struct ChangesForGenerator {
    /// Changes written since the last message, in every chunk (filtered when sent).
    pending: Vec<BlockChange>,
    /// Number of the last message sent (0: none yet).
    last_sent: u64,
    /// Positions sent, with their message number, that the thread's outputs still to come may lack.
    sent_not_applied: Vec<(u64, [i32; 3])>,
    /// Last message whose changes a save wrote, set by the write thread, read by the generation thread.
    pub(crate) saved: Arc<AtomicU64>,
    /// Positions the server relit itself because they reached the thread too late (since opening).
    pub(crate) relit_by_server: u64,
}

impl ChangesForGenerator {
    /// A block written by the server (any chunk).
    pub(crate) fn record(&mut self, p: [i32; 3], state: u32) {
        self.pending.push((p, state));
    }

    /// The next message: the pending changes of the chunks the thread may still read
    /// (`full_on_server`: chunk loaded full by the server), numbered; `None` when there is none.
    /// `RUSTCRAFT_OPT_SEND_ONLY_FRONTIER_BLOCK_CHANGES=0`: every change is sent (the thread then
    /// applies and keeps until the next save changes it will never read).
    pub(crate) fn next_message(
        &mut self,
        full_on_server: impl Fn((i32, i32)) -> bool,
    ) -> Option<(u64, Vec<BlockChange>)> {
        let frontier_only = rc_data::opt_switch!("RUSTCRAFT_OPT_SEND_ONLY_FRONTIER_BLOCK_CHANGES");
        self.next_message_with(frontier_only, full_on_server)
    }

    /// [`Self::next_message`] with the frontier filter given.
    fn next_message_with(
        &mut self,
        frontier_only: bool,
        full_on_server: impl Fn((i32, i32)) -> bool,
    ) -> Option<(u64, Vec<BlockChange>)> {
        if self.pending.is_empty() {
            return None;
        }
        // One neighbourhood check per chunk, not per change: a fill or a flood writes thousands. The
        // last chunk's answer is kept apart: writes come in runs within one chunk, which then skip
        // the map lookup.
        let mut readable: HashMap<(i32, i32), bool> = HashMap::new();
        let mut last: Option<((i32, i32), bool)> = None;
        let mut may_read = |chunk: (i32, i32)| match last {
            Some((c, answer)) if c == chunk => answer,
            _ => {
                let answer = *readable.entry(chunk).or_insert_with_key(|&c| generator_may_read(c, &full_on_server));
                last = Some((chunk, answer));
                answer
            }
        };
        let changes: Vec<BlockChange> =
            self.pending.drain(..).filter(|(p, _)| !frontier_only || may_read((p[0] >> 4, p[2] >> 4))).collect();
        if changes.is_empty() {
            return None;
        }
        self.last_sent += 1;
        let number = self.last_sent;
        self.sent_not_applied.extend(changes.iter().map(|&(p, _)| (number, p)));
        Some((number, changes))
    }

    /// Number of the last message sent: a save started now writes every change up to it.
    pub(crate) fn last_sent(&self) -> u64 {
        self.last_sent
    }

    /// The thread's outputs that follow include every message up to `number`.
    pub(crate) fn thread_applied(&mut self, number: u64) {
        self.sent_not_applied.retain(|&(n, _)| n > number);
    }

    /// Positions sent that the light of chunk `(x, z)` may lack: those of its 3 × 3.
    pub(crate) fn sent_not_applied_around(&self, (x, z): (i32, i32), out: &mut Vec<[i32; 3]>) {
        let near = |p: &[i32; 3]| ((p[0] >> 4) - x).abs() <= 1 && ((p[2] >> 4) - z).abs() <= 1;
        out.extend(self.sent_not_applied.iter().map(|&(_, p)| p).filter(near));
    }

    /// Positions sent and not yet covered by an `Out::ChangesApplied` (tests).
    #[cfg(test)]
    pub(crate) fn sent_not_applied_count(&self) -> usize {
        self.sent_not_applied.len()
    }
}

/// The generation thread may still read chunk `(x, z)`: one of the 25 chunks within two of it is
/// not full on the server, so it may still be made full (which reads its 3 × 3) and relight its
/// full neighbours (which reads theirs).
fn generator_may_read((x, z): (i32, i32), full_on_server: impl Fn((i32, i32)) -> bool) -> bool {
    (-2..=2).any(|dz| (-2..=2).any(|dx| !full_on_server((x + dx, z + dz))))
}

/// Thread side: the server's changes received.
#[derive(Default)]
pub(crate) struct ServerChanges {
    /// Last message applied.
    applied: u64,
    /// Last message reported to the server (`Out::ChangesApplied`).
    reported: u64,
    /// Changes no save has written yet, per chunk, in arrival order. Applied again, in order, to a
    /// chunk that comes back (set aside or read from disk): whatever older version it comes back as,
    /// they bring it to the server's current blocks.
    not_on_disk: HashMap<(i32, i32), Vec<KeptChange>>,
    /// Last message a save wrote (shared with the server's write thread).
    pub(crate) saved: Arc<AtomicU64>,
}

/// A server change kept for its chunk.
struct KeptChange {
    /// Message that brought it.
    number: u64,
    /// Block index in the chunk (y, then z, then x, like the generator's chunks).
    index: u32,
    state: u32,
}

impl ServerChanges {
    /// Chunks with changes no save has written yet (tests).
    #[cfg(test)]
    pub(crate) fn chunks_not_on_disk(&self) -> usize {
        self.not_on_disk.len()
    }
}

impl Worker {
    /// Applies a message of server changes to the chunks held, and keeps it until a save writes it.
    pub(super) fn apply_server_changes(&mut self, number: u64, changes: Vec<BlockChange>) {
        self.forget_saved_server_changes();
        let (min_y, height) = (self.height.min_y, self.height.sections as i32 * 16);
        for (p, state) in changes {
            let y = p[1] - min_y;
            if !(0..height).contains(&y) {
                continue;
            }
            let chunk = (p[0] >> 4, p[2] >> 4);
            let index = ((y << 8) | ((p[2] & 15) << 4) | (p[0] & 15)) as u32;
            if let Some(s) = self.map.get_mut(&chunk) {
                set_block(&mut s.c.blocks, index, state);
            }
            self.server_changes.not_on_disk.entry(chunk).or_default().push(KeptChange { number, index, state });
        }
        self.server_changes.applied = number;
    }

    /// Chunk `p` back in the table (set aside, or read from disk): the server changes it may lack.
    pub(super) fn catch_up_server_changes(&mut self, p: (i32, i32)) {
        let (Some(changes), Some(s)) = (self.server_changes.not_on_disk.get(&p), self.map.get_mut(&p)) else {
            return;
        };
        for change in changes {
            set_block(&mut s.c.blocks, change.index, change.state);
        }
    }

    /// Forgets the changes a save wrote, except for the chunks set aside: their copy, taken before
    /// the save, may lack them, and they come back from it, not from disk.
    pub(super) fn forget_saved_server_changes(&mut self) {
        let saved = self.server_changes.saved.load(Ordering::Acquire);
        let parked = &self.parked;
        self.server_changes.not_on_disk.retain(|p, changes| {
            if parked.contains_key(p) {
                return true;
            }
            changes.retain(|change| change.number > saved);
            !changes.is_empty()
        });
    }

    /// Tells the server the last message applied, when it has not been told yet: the outputs that
    /// follow include it.
    pub(super) fn report_server_changes_seen(&mut self) {
        let changes = &mut self.server_changes;
        if changes.applied > changes.reported {
            changes.reported = changes.applied;
            let _ = self.out.send(Out::ChangesApplied(changes.applied));
        }
    }
}

fn set_block(blocks: &mut [u32], index: u32, state: u32) {
    if let Some(b) = blocks.get_mut(index as usize) {
        *b = state;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Server map where `full` lists the chunks loaded full.
    fn full_in(full: &[(i32, i32)]) -> impl Fn((i32, i32)) -> bool + '_ {
        move |p| full.contains(&p)
    }

    /// A change is sent only while one of the 25 chunks within two of its chunk is not full on the
    /// server; the check runs per chunk, the changes keep their order.
    #[test]
    fn only_changes_the_generator_may_still_read_are_sent() {
        let all: Vec<(i32, i32)> = (-2..=2).flat_map(|x| (-2..=2).map(move |z| (x, z))).collect();
        let mut changes = ChangesForGenerator::default();
        changes.record([5, 64, 5], 1);
        assert!(changes.next_message_with(true, full_in(&all)).is_none(), "chunk surrounded by full chunks sent");
        let mut missing_corner = all.clone();
        missing_corner.retain(|&p| p != (2, -2));
        changes.record([5, 64, 5], 2);
        changes.record([40, 64, 5], 3);
        changes.record([6, 70, 5], 4);
        let (number, sent) = changes.next_message_with(true, full_in(&missing_corner)).expect("frontier chunk");
        assert_eq!(number, 1);
        assert_eq!(sent, vec![([5, 64, 5], 2), ([40, 64, 5], 3), ([6, 70, 5], 4)]);
        assert_eq!(changes.last_sent(), 1);
        // Filter off: the change deep inside the loaded area is sent too.
        changes.record([5, 64, 5], 5);
        assert_eq!(changes.next_message_with(false, full_in(&all)), Some((2, vec![([5, 64, 5], 5)])));
    }

    /// Micro-bench (run by hand, `--ignored --nocapture`): cost per change of a tick's message, for a
    /// fill of 98 304 blocks over 24 chunks deep inside the loaded area (nothing sent) and at the
    /// frontier (everything sent), filter on, then off.
    #[test]
    #[ignore]
    fn bench_next_message() {
        let changes_per_tick: Vec<[i32; 3]> =
            (0..24).flat_map(|c| (0..4096).map(move |i| [c * 16 + (i & 15), i >> 8, (i >> 4) & 15])).collect();
        let loaded: hashbrown::HashSet<(i32, i32)> = (-3..30).flat_map(|x| (-3..3).map(move |z| (x, z))).collect();
        for frontier_only in [true, false] {
            for (place, full) in [("inside", 0), ("frontier", 1)] {
                let mut changes = ChangesForGenerator::default();
                let t = std::time::Instant::now();
                let rounds = 20;
                for _ in 0..rounds {
                    for &p in &changes_per_tick {
                        changes.record(p, 1);
                    }
                    let is_full = |c: (i32, i32)| full == 0 && loaded.contains(&c);
                    let _ = changes.next_message_with(frontier_only, is_full);
                    changes.thread_applied(changes.last_sent());
                }
                let ns = t.elapsed().as_nanos() as f64 / (rounds * changes_per_tick.len()) as f64;
                eprintln!("filter {frontier_only}, fill {place}: {ns:.1} ns per change (record + message)");
            }
        }
    }

    /// The positions sent stay "unseen" until the thread reports their message; only those of the
    /// 3 × 3 of a chunk are returned for it.
    #[test]
    fn unseen_positions_follow_the_reports() {
        let mut changes = ChangesForGenerator::default();
        changes.record([5, 64, 5], 1);
        let _ = changes.next_message(|_| false);
        changes.record([40, 64, 5], 2);
        let _ = changes.next_message(|_| false);
        // [5, 64, 5] is in chunk (0, 0), [40, 64, 5] in chunk (2, 0).
        let mut out = Vec::new();
        changes.sent_not_applied_around((-1, 0), &mut out);
        assert_eq!(out, vec![[5, 64, 5]]);
        out.clear();
        changes.sent_not_applied_around((3, 0), &mut out);
        assert_eq!(out, vec![[40, 64, 5]]);
        changes.thread_applied(1);
        assert_eq!(changes.sent_not_applied_count(), 1);
        changes.thread_applied(2);
        assert_eq!(changes.sent_not_applied_count(), 0);
    }
}
