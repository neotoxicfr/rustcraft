//! Allocation of a GPU buffer into element ranges: first fit, neighbouring holes
//! merged, growth at the end. Bookkeeping only (the buffer lives in the renderer).

/// Free ranges of a buffer of `capacity` elements.
pub struct Arena {
    pub capacity: u32,
    /// (start, length), sorted by start, never adjacent.
    free: Vec<(u32, u32)>,
}

impl Arena {
    pub fn new(capacity: u32) -> Self {
        Arena { capacity, free: if capacity > 0 { vec![(0, capacity)] } else { Vec::new() } }
    }

    /// Start of a range of `n` elements, or `None` if the buffer must grow.
    pub fn alloc(&mut self, n: u32) -> Option<u32> {
        if n == 0 {
            return Some(0);
        }
        let i = self.free.iter().position(|&(_, len)| len >= n)?;
        let (start, len) = self.free[i];
        if len == n {
            self.free.remove(i);
        } else {
            self.free[i] = (start + n, len - n);
        }
        Some(start)
    }

    pub fn release(&mut self, start: u32, n: u32) {
        if n == 0 {
            return;
        }
        let i = self.free.partition_point(|&(s, _)| s < start);
        self.free.insert(i, (start, n));
        // Merge with the next range, then with the previous one.
        if i + 1 < self.free.len() && self.free[i].0 + self.free[i].1 == self.free[i + 1].0 {
            self.free[i].1 += self.free[i + 1].1;
            self.free.remove(i + 1);
        }
        if i > 0 && self.free[i - 1].0 + self.free[i - 1].1 == self.free[i].0 {
            self.free[i - 1].1 += self.free[i].1;
            self.free.remove(i);
        }
    }

    /// New (larger) capacity: the added end becomes free.
    pub fn grow(&mut self, capacity: u32) {
        let old = self.capacity;
        self.capacity = capacity;
        self.release(old, capacity - old);
    }

    /// Free elements (checks).
    #[cfg(test)]
    pub fn free_total(&self) -> u32 {
        self.free.iter().map(|f| f.1).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_fit_merge_and_grow() {
        let mut a = Arena::new(100);
        assert_eq!(a.alloc(30), Some(0));
        assert_eq!(a.alloc(30), Some(30));
        assert_eq!(a.alloc(30), Some(60));
        assert_eq!(a.alloc(30), None);
        a.release(30, 30);
        a.release(0, 30);
        assert_eq!(a.free_total(), 70);
        // Hole 0..60 (two merged releases): 60 fits at the start.
        assert_eq!(a.alloc(60), Some(0));
        // The hole 90..100 merges with the added end: 100 elements from 90.
        a.grow(200);
        assert_eq!(a.alloc(100), Some(90));
        assert_eq!(a.free_total(), 10);
        a.release(60, 30);
        a.release(0, 60);
        a.release(90, 100);
        assert_eq!(a.free_total(), 200);
        assert_eq!(a.alloc(200), Some(0));
    }

    #[test]
    fn random_operations_keep_free_list_consistent() {
        let mut seed = 7u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut a = Arena::new(1000);
        let mut live: Vec<(u32, u32)> = Vec::new();
        for _ in 0..5000 {
            if next() % 2 == 0 || live.is_empty() {
                let n = (next() % 50) as u32 + 1;
                match a.alloc(n) {
                    Some(s) => {
                        assert!(live.iter().all(|&(t, m)| s + n <= t || t + m <= s), "overlap");
                        live.push((s, n));
                    }
                    None => a.grow(a.capacity * 2),
                }
            } else {
                let (s, n) = live.swap_remove((next() % live.len() as u64) as usize);
                a.release(s, n);
            }
            let used: u32 = live.iter().map(|l| l.1).sum();
            assert_eq!(used + a.free_total(), a.capacity);
        }
    }
}
