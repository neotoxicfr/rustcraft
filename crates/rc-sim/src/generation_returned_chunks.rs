//! Chunks the generation thread has returned full since it started. They are never forgotten: one
//! read back from disk stays the thread's (relit with its new neighbours) and counts as full for the
//! eviction wherever it is, so neither bounding nor emptying the record would keep the result. Held as
//! one bit per chunk in a bitmap per region of 32 × 32 chunks (`RUSTCRAFT_OPT_RETURNED_CHUNKS_BITMAP`,
//! default on): a long exploration costs a fraction of a byte per chunk instead of the dozen or so of
//! a hash set entry (`0`: the hash set).

use hashbrown::{HashMap, HashSet};

/// Bits of a region: chunk `(x & 31, z & 31)` at bit `(z & 31) · 32 + (x & 31)`.
type RegionBits = [u64; 16];

pub(crate) enum ReturnedChunks {
    Set(HashSet<(i32, i32)>),
    /// Bitmaps per region `(x >> 5, z >> 5)`, and the number of chunks recorded.
    Regions(HashMap<(i32, i32), RegionBits>, usize),
}

/// Region of chunk `(x, z)`, and the word and bit of the chunk in its bitmap.
fn place((x, z): (i32, i32)) -> ((i32, i32), usize, u64) {
    let bit = ((z & 31) * 32 + (x & 31)) as usize;
    ((x >> 5, z >> 5), bit / 64, 1 << (bit % 64))
}

impl ReturnedChunks {
    /// Empty record, as bitmaps per region or as a hash set.
    pub(crate) fn new(bitmaps: bool) -> ReturnedChunks {
        if bitmaps { ReturnedChunks::Regions(HashMap::new(), 0) } else { ReturnedChunks::Set(HashSet::new()) }
    }

    pub(crate) fn insert(&mut self, p: (i32, i32)) {
        match self {
            ReturnedChunks::Set(s) => {
                s.insert(p);
            }
            ReturnedChunks::Regions(regions, len) => {
                let (region, word, bit) = place(p);
                let bits = &mut regions.entry(region).or_insert([0; 16])[word];
                *len += usize::from(*bits & bit == 0);
                *bits |= bit;
            }
        }
    }

    pub(crate) fn contains(&self, p: (i32, i32)) -> bool {
        match self {
            ReturnedChunks::Set(s) => s.contains(&p),
            ReturnedChunks::Regions(regions, _) => {
                let (region, word, bit) = place(p);
                regions.get(&region).is_some_and(|b| b[word] & bit != 0)
            }
        }
    }

    /// Chunks recorded.
    pub(crate) fn len(&self) -> usize {
        match self {
            ReturnedChunks::Set(s) => s.len(),
            ReturnedChunks::Regions(_, len) => *len,
        }
    }

    /// Bytes of the record's table as allocated (memory statistics).
    pub(crate) fn heap_bytes(&self) -> usize {
        match self {
            ReturnedChunks::Set(s) => table_bytes(s.capacity(), size_of::<(i32, i32)>()),
            ReturnedChunks::Regions(regions, _) => table_bytes(regions.capacity(), size_of::<((i32, i32), RegionBits)>()),
        }
    }
}

/// Bytes of a hashbrown table that holds `capacity` entries of `entry` bytes: its buckets (a power of
/// two, at most 7/8 full), one control byte per bucket and one group of control bytes more.
fn table_bytes(capacity: usize, entry: usize) -> usize {
    if capacity == 0 {
        return 0;
    }
    let buckets = if capacity < 8 { (capacity + 1).next_power_of_two() } else { (capacity * 8 / 7).next_power_of_two() };
    buckets * (entry + 1) + 16
}

#[cfg(test)]
mod tests {
    use super::ReturnedChunks;

    /// Both records answer alike for chunks on both sides of zero and of region edges, inserted
    /// twice or not at all.
    #[test]
    fn bitmaps_answer_like_the_hash_set() {
        let (mut set, mut bitmaps) = (ReturnedChunks::new(false), ReturnedChunks::new(true));
        let mut seed = 0x5eed_u64;
        let mut next = || {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            ((seed >> 33) % 200) as i32 - 100
        };
        for _ in 0..3_000 {
            let p = (next(), next());
            set.insert(p);
            bitmaps.insert(p);
        }
        for p in [(31, 31), (32, 32), (-1, -1), (-32, -33), (i32::MAX, i32::MIN)] {
            set.insert(p);
            bitmaps.insert(p);
        }
        assert_eq!(set.len(), bitmaps.len());
        for z in -110..110 {
            for x in -110..110 {
                assert_eq!(set.contains((x, z)), bitmaps.contains((x, z)), "chunk {x} {z}");
            }
        }
        assert!(bitmaps.contains((i32::MAX, i32::MIN)) && !bitmaps.contains((i32::MIN, i32::MAX)));
    }

    /// An explored square of 128 × 128 chunks (16 regions): the bitmaps take a fraction of a byte per
    /// chunk, the hash set more than ten.
    #[test]
    fn bitmaps_of_an_explored_area_cost_a_fraction_of_the_hash_set() {
        let (mut set, mut bitmaps) = (ReturnedChunks::new(false), ReturnedChunks::new(true));
        for z in -64..64 {
            for x in -64..64 {
                set.insert((x, z));
                bitmaps.insert((x, z));
            }
        }
        let chunks = 128.0 * 128.0;
        let (set_per_chunk, bitmap_per_chunk) =
            (set.heap_bytes() as f64 / chunks, bitmaps.heap_bytes() as f64 / chunks);
        eprintln!("bytes per chunk: hash set {set_per_chunk:.2}, bitmaps {bitmap_per_chunk:.3}");
        assert!(set_per_chunk > 10.0 && bitmap_per_chunk < 0.5, "{set_per_chunk} {bitmap_per_chunk}");
    }
}
