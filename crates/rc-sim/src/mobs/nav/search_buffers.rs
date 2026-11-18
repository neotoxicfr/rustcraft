//! Working memory of a path search: node arena, per-position tables, heap and neighbour lists.
//! With `RUSTCRAFT_OPT_PATH_SCRATCH` one set is kept per thread and handed from search to
//! search, emptied but with its capacity (a village asks for hundreds of searches per second;
//! fresh tables cost an allocation each time they grow).

use std::cell::RefCell;

use hashbrown::HashMap;

use super::{Node, PathType};

/// Key of a cell (or a section) relative to the search origin: 21 bits per axis, enough for any
/// cell a search can reach (within the follow range, at most 2,048, plus a fall or a world
/// height of the origin), where three `i32` would need 96 bits.
pub(super) fn relative_key(origin: [i32; 3], x: i32, y: i32, z: i32) -> u64 {
    const MASK: u64 = (1 << 21) - 1;
    let field = |v: i32, o: i32| (v.wrapping_sub(o) as u32 as u64) & MASK;
    (field(x, origin[0]) << 42) | (field(y, origin[1]) << 21) | field(z, origin[2])
}

/// How the memo of a search keeps its path types (`RUSTCRAFT_OPT_PATH_MEMO`: 0, 1, 2 by default).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemoKind {
    Off,
    /// Hash map of every position read (level 1).
    HashMap,
    /// Direct-mapped table of 4,096 slots (level 2): the slot comes from the key, the key is
    /// checked; a search reads the same few thousand cells again and again, and a slot read
    /// costs a fraction of a hash map lookup. Two keys on one slot only evict each other (the
    /// type is computed again).
    DirectMapped,
}

const MEMO_SLOT_BITS: u32 = 12;

/// Path types of one search per position, under `MemoKind`.
#[derive(Default)]
pub(super) struct TypeMemo {
    map: HashMap<u64, PathType>,
    /// (key, search stamp, type); a slot of an older search has an older stamp.
    slots: Vec<(u64, u32, PathType)>,
    stamp: u32,
}

impl TypeMemo {
    /// Forgets the previous search (the slots are not cleared: their stamp no longer matches).
    fn clear(&mut self) {
        self.map.clear();
        self.stamp = self.stamp.wrapping_add(1);
        if self.stamp == 0 {
            self.slots.clear();
            self.stamp = 1;
        }
    }

    fn slot(key: u64) -> usize {
        (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> (64 - MEMO_SLOT_BITS)) as usize
    }

    pub(super) fn get(&self, kind: MemoKind, key: u64) -> Option<PathType> {
        match kind {
            MemoKind::Off => None,
            MemoKind::HashMap => self.map.get(&key).copied(),
            MemoKind::DirectMapped => {
                let &(k, stamp, t) = self.slots.get(Self::slot(key))?;
                (k == key && stamp == self.stamp).then_some(t)
            }
        }
    }

    pub(super) fn insert(&mut self, kind: MemoKind, key: u64, t: PathType) {
        match kind {
            MemoKind::Off => {}
            MemoKind::HashMap => {
                self.map.insert(key, t);
            }
            MemoKind::DirectMapped => {
                if self.slots.is_empty() {
                    // Stamp 0 is never a search's: the fresh slots hold nothing.
                    self.slots = vec![(0, 0, PathType::Blocked); 1 << MEMO_SLOT_BITS];
                    self.stamp = self.stamp.max(1);
                }
                self.slots[Self::slot(key)] = (key, self.stamp, t);
            }
        }
    }
}

#[derive(Default)]
pub(super) struct SearchBuffers {
    pub(super) nodes: Vec<Node>,
    pub(super) index: HashMap<u64, usize>,
    /// Type of a cell for the whole footprint of the mob.
    pub(super) footprint_types: HashMap<u64, PathType>,
    pub(super) collisions: HashMap<[u64; 6], bool>,
    /// Memo of the search (`RUSTCRAFT_OPT_PATH_MEMO`): type of a single block, static type of a cell.
    pub(super) block_types: TypeMemo,
    pub(super) static_types: TypeMemo,
    /// Sections whose palette may hold a dangerous block (`RUSTCRAFT_OPT_PATH_PALETTE`).
    pub(super) section_danger: HashMap<u64, bool>,
    pub(super) heap: Vec<usize>,
    pub(super) neighbours: Vec<usize>,
    pub(super) target_bests: Vec<(f32, usize)>,
    pub(super) target_reached: Vec<bool>,
}

impl SearchBuffers {
    fn clear(&mut self) {
        self.nodes.clear();
        self.index.clear();
        self.footprint_types.clear();
        self.collisions.clear();
        self.block_types.clear();
        self.static_types.clear();
        self.section_danger.clear();
        self.heap.clear();
        self.neighbours.clear();
        self.target_bests.clear();
        self.target_reached.clear();
    }
}

thread_local! {
    static KEPT: RefCell<SearchBuffers> = RefCell::new(SearchBuffers::default());
}

/// The buffers kept by this thread, emptied (fresh ones if a search is already running here).
pub(super) fn take_kept() -> SearchBuffers {
    let mut b = KEPT.take();
    b.clear();
    b
}

/// Hands the buffers back to this thread for the next search.
pub(super) fn keep(b: SearchBuffers) {
    KEPT.set(b);
}
