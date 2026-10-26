//! Set of positions iterated in the exact order of a Java hash set (buckets by increasing
//! index; initial capacity 16, factor 0.75; hash of the game's positions).
//!
//! Order within a bucket: that of its linked list. As long as a bucket is a list, it is the
//! insertion order. A bucket that exceeds 8 elements in a table of at least 64 becomes a
//! red-black tree (Java): the linked list is kept, but the root of the tree moves to the head, and
//! each new element is placed right behind its parent in the tree. We therefore reproduce the tree
//! (comparison by signed hash, same rotations), its root put back at the head, its splitting at
//! table doubling (back to a list at 6 elements or fewer, rebuilt otherwise) and removal
//! through the iterator (tree rebalanced, root left in place).
//!
//! Only possible difference: two positions with the same full hash in the same tree; Java breaks
//! the tie by object address (non-reproducible order, from one run to the next), we always take
//! the left. It takes a gap of 31 blocks in x or in z between two positions of the same
//! set, out of reach of a tree.
//!
//! Switch `RUSTCRAFT_OPT_JSET_KEEP` (default: new): buckets kept from one clear to the next,
//! in-place doubling, cursor of the first non-empty bucket for removal; `=0`: old path
//! (table brought back to 16 buckets at clear, doubling by copy, removal searched from bucket 0).

use std::sync::atomic::{AtomicUsize, Ordering};

const NIL: usize = usize::MAX;
const TREEIFY: usize = 8;
const UNTREEIFY: usize = 6;
const MIN_TREE_CAP: usize = 64;

static TREEIFIED: AtomicUsize = AtomicUsize::new(0);

/// Number of buckets that became trees since launch (measurement, tests).
pub fn treeified_count() -> usize {
    TREEIFIED.load(Ordering::Relaxed)
}

/// Hash of a position (the game's), then mixing of the 16 high bits like Java.
#[inline]
pub fn jhash(p: [i32; 3]) -> u32 {
    let h = p[1].wrapping_add(p[2].wrapping_mul(31)).wrapping_mul(31).wrapping_add(p[0]) as u32;
    h ^ (h >> 16)
}

#[derive(Debug, Clone, Copy)]
struct Node {
    key: [i32; 3],
    hash: i32,
    parent: usize,
    left: usize,
    right: usize,
    red: bool,
}

/// Tree of a bucket: nodes (removed ones stay, detached) and root.
#[derive(Debug, Clone)]
struct TreeBin {
    bucket: usize,
    n: Vec<Node>,
    root: usize,
}

impl TreeBin {
    /// Builds the tree of the elements of `list` taken in order, then puts the root at the head.
    fn build(bucket: usize, list: &mut [[i32; 3]]) -> TreeBin {
        TREEIFIED.fetch_add(1, Ordering::Relaxed);
        let mut t = TreeBin { bucket, n: Vec::with_capacity(list.len() + 4), root: NIL };
        for &k in list.iter() {
            let x = t.n.len();
            t.n.push(Node { key: k, hash: jhash(k) as i32, parent: NIL, left: NIL, right: NIL, red: false });
            if t.root == NIL {
                t.root = x;
                continue;
            }
            let xp = t.descend(t.n[x].hash);
            t.attach(xp, x);
            t.root = t.balance_insertion(t.root, x);
        }
        let rk = t.n[t.root].key;
        move_to_front(list, rk);
        t
    }

    /// Leaf under which a hash `h` attaches (tie: to the left, see the header).
    fn descend(&self, h: i32) -> usize {
        let mut p = self.root;
        loop {
            let left = self.n[p].hash >= h;
            let c = if left { self.n[p].left } else { self.n[p].right };
            if c == NIL {
                return p;
            }
            p = c;
        }
    }

    fn attach(&mut self, xp: usize, x: usize) {
        self.n[x].parent = xp;
        if self.n[xp].hash >= self.n[x].hash {
            self.n[xp].left = x;
        } else {
            self.n[xp].right = x;
        }
    }

    /// Addition of an absent key: placed behind its parent in the list, root put back at the head.
    fn insert(&mut self, list: &mut Vec<[i32; 3]>, k: [i32; 3]) {
        let x = self.n.len();
        self.n.push(Node { key: k, hash: jhash(k) as i32, parent: NIL, left: NIL, right: NIL, red: false });
        let xp = self.descend(self.n[x].hash);
        self.attach(xp, x);
        let at = list.iter().position(|q| *q == self.n[xp].key).expect("parent in the list");
        list.insert(at + 1, k);
        let r = self.balance_insertion(self.root, x);
        self.root = r;
        move_to_front(list, self.n[r].key);
    }

    /// Removal of the list head by the iterator (already taken off `list`): tree rebalanced,
    /// root left where it is.
    fn remove(&mut self, k: [i32; 3]) {
        let p = (0..self.n.len()).find(|&i| self.n[i].key == k && self.linked(i)).expect("tree node");
        let mut root = self.root;
        let (pl, pr) = (self.n[p].left, self.n[p].right);
        let replacement = if pl != NIL && pr != NIL {
            let mut s = pr;
            while self.n[s].left != NIL {
                s = self.n[s].left;
            }
            let c = self.n[s].red;
            self.n[s].red = self.n[p].red;
            self.n[p].red = c;
            let sr = self.n[s].right;
            let pp = self.n[p].parent;
            if s == pr {
                self.n[p].parent = s;
                self.n[s].right = p;
            } else {
                let sp = self.n[s].parent;
                self.n[p].parent = sp;
                if sp != NIL {
                    if s == self.n[sp].left {
                        self.n[sp].left = p;
                    } else {
                        self.n[sp].right = p;
                    }
                }
                self.n[s].right = pr;
                if pr != NIL {
                    self.n[pr].parent = s;
                }
            }
            self.n[p].left = NIL;
            self.n[p].right = sr;
            if sr != NIL {
                self.n[sr].parent = p;
            }
            self.n[s].left = pl;
            if pl != NIL {
                self.n[pl].parent = s;
            }
            self.n[s].parent = pp;
            if pp == NIL {
                root = s;
            } else if p == self.n[pp].left {
                self.n[pp].left = s;
            } else {
                self.n[pp].right = s;
            }
            if sr != NIL { sr } else { p }
        } else if pl != NIL {
            pl
        } else if pr != NIL {
            pr
        } else {
            p
        };
        if replacement != p {
            let pp = self.n[p].parent;
            self.n[replacement].parent = pp;
            if pp == NIL {
                root = replacement;
                self.n[replacement].red = false;
            } else if p == self.n[pp].left {
                self.n[pp].left = replacement;
            } else {
                self.n[pp].right = replacement;
            }
            self.n[p].left = NIL;
            self.n[p].right = NIL;
            self.n[p].parent = NIL;
        }
        let r = if self.n[p].red { root } else { self.balance_deletion(root, replacement) };
        if replacement == p {
            let pp = self.n[p].parent;
            self.n[p].parent = NIL;
            if pp != NIL {
                if p == self.n[pp].left {
                    self.n[pp].left = NIL;
                } else if p == self.n[pp].right {
                    self.n[pp].right = NIL;
                }
            }
        }
        // The removed node is marked out of the tree (hash kept, no link any more).
        self.n[p].key = [i32::MIN; 3];
        let mut r = r;
        while self.n[r].parent != NIL {
            r = self.n[r].parent;
        }
        self.root = r;
    }

    /// Node still in the tree (removed ones have a sentinel key).
    fn linked(&self, i: usize) -> bool {
        self.n[i].key != [i32::MIN; 3]
    }

    fn rotate_left(&mut self, mut root: usize, p: usize) -> usize {
        if p != NIL {
            let r = self.n[p].right;
            if r != NIL {
                let rl = self.n[r].left;
                self.n[p].right = rl;
                if rl != NIL {
                    self.n[rl].parent = p;
                }
                let pp = self.n[p].parent;
                self.n[r].parent = pp;
                if pp == NIL {
                    root = r;
                    self.n[r].red = false;
                } else if self.n[pp].left == p {
                    self.n[pp].left = r;
                } else {
                    self.n[pp].right = r;
                }
                self.n[r].left = p;
                self.n[p].parent = r;
            }
        }
        root
    }

    fn rotate_right(&mut self, mut root: usize, p: usize) -> usize {
        if p != NIL {
            let l = self.n[p].left;
            if l != NIL {
                let lr = self.n[l].right;
                self.n[p].left = lr;
                if lr != NIL {
                    self.n[lr].parent = p;
                }
                let pp = self.n[p].parent;
                self.n[l].parent = pp;
                if pp == NIL {
                    root = l;
                    self.n[l].red = false;
                } else if self.n[pp].right == p {
                    self.n[pp].right = l;
                } else {
                    self.n[pp].left = l;
                }
                self.n[l].right = p;
                self.n[p].parent = l;
            }
        }
        root
    }

    fn red(&self, i: usize) -> bool {
        i != NIL && self.n[i].red
    }

    fn balance_insertion(&mut self, mut root: usize, mut x: usize) -> usize {
        self.n[x].red = true;
        loop {
            let mut xp = self.n[x].parent;
            if xp == NIL {
                self.n[x].red = false;
                return x;
            }
            if !self.n[xp].red {
                return root;
            }
            let mut xpp = self.n[xp].parent;
            if xpp == NIL {
                return root;
            }
            let xppl = self.n[xpp].left;
            if xp == xppl {
                let xppr = self.n[xpp].right;
                if self.red(xppr) {
                    self.n[xppr].red = false;
                    self.n[xp].red = false;
                    self.n[xpp].red = true;
                    x = xpp;
                } else {
                    if x == self.n[xp].right {
                        x = xp;
                        root = self.rotate_left(root, x);
                        xp = self.n[x].parent;
                        xpp = if xp == NIL { NIL } else { self.n[xp].parent };
                    }
                    if xp != NIL {
                        self.n[xp].red = false;
                        if xpp != NIL {
                            self.n[xpp].red = true;
                            root = self.rotate_right(root, xpp);
                        }
                    }
                }
            } else if self.red(xppl) {
                self.n[xppl].red = false;
                self.n[xp].red = false;
                self.n[xpp].red = true;
                x = xpp;
            } else {
                if x == self.n[xp].left {
                    x = xp;
                    root = self.rotate_right(root, x);
                    xp = self.n[x].parent;
                    xpp = if xp == NIL { NIL } else { self.n[xp].parent };
                }
                if xp != NIL {
                    self.n[xp].red = false;
                    if xpp != NIL {
                        self.n[xpp].red = true;
                        root = self.rotate_left(root, xpp);
                    }
                }
            }
        }
    }

    fn balance_deletion(&mut self, mut root: usize, mut x: usize) -> usize {
        loop {
            if x == NIL || x == root {
                return root;
            }
            let mut xp = self.n[x].parent;
            if xp == NIL {
                self.n[x].red = false;
                return x;
            }
            if self.n[x].red {
                self.n[x].red = false;
                return root;
            }
            let xpl = self.n[xp].left;
            if xpl == x {
                let mut xpr = self.n[xp].right;
                if self.red(xpr) {
                    self.n[xpr].red = false;
                    self.n[xp].red = true;
                    root = self.rotate_left(root, xp);
                    xp = self.n[x].parent;
                    xpr = if xp == NIL { NIL } else { self.n[xp].right };
                }
                if xpr == NIL {
                    x = xp;
                } else {
                    let (sl, mut sr) = (self.n[xpr].left, self.n[xpr].right);
                    if !self.red(sr) && !self.red(sl) {
                        self.n[xpr].red = true;
                        x = xp;
                    } else {
                        if !self.red(sr) {
                            if sl != NIL {
                                self.n[sl].red = false;
                            }
                            self.n[xpr].red = true;
                            root = self.rotate_right(root, xpr);
                            xp = self.n[x].parent;
                            xpr = if xp == NIL { NIL } else { self.n[xp].right };
                        }
                        if xpr != NIL {
                            self.n[xpr].red = if xp == NIL { false } else { self.n[xp].red };
                            sr = self.n[xpr].right;
                            if sr != NIL {
                                self.n[sr].red = false;
                            }
                        }
                        if xp != NIL {
                            self.n[xp].red = false;
                            root = self.rotate_left(root, xp);
                        }
                        x = root;
                    }
                }
            } else {
                let mut xpl = xpl;
                if self.red(xpl) {
                    self.n[xpl].red = false;
                    self.n[xp].red = true;
                    root = self.rotate_right(root, xp);
                    xp = self.n[x].parent;
                    xpl = if xp == NIL { NIL } else { self.n[xp].left };
                }
                if xpl == NIL {
                    x = xp;
                } else {
                    let (mut sl, sr) = (self.n[xpl].left, self.n[xpl].right);
                    if !self.red(sl) && !self.red(sr) {
                        self.n[xpl].red = true;
                        x = xp;
                    } else {
                        if !self.red(sl) {
                            if sr != NIL {
                                self.n[sr].red = false;
                            }
                            self.n[xpl].red = true;
                            root = self.rotate_left(root, xpl);
                            xp = self.n[x].parent;
                            xpl = if xp == NIL { NIL } else { self.n[xp].left };
                        }
                        if xpl != NIL {
                            self.n[xpl].red = if xp == NIL { false } else { self.n[xp].red };
                            sl = self.n[xpl].left;
                            if sl != NIL {
                                self.n[sl].red = false;
                            }
                        }
                        if xp != NIL {
                            self.n[xp].red = false;
                            root = self.rotate_right(root, xp);
                        }
                        x = root;
                    }
                }
            }
        }
    }
}

fn move_to_front(list: &mut [[i32; 3]], k: [i32; 3]) {
    if let Some(i) = list.iter().position(|q| *q == k) {
        list[..=i].rotate_right(1);
    }
}

/// See the module header.
#[derive(Debug)]
pub struct JSet {
    /// Buckets (linked list of each, in order); only the first `cap` are used, the
    /// following ones (kept from a previous clear) are empty.
    buckets: Vec<Vec<[i32; 3]>>,
    cap: usize,
    len: usize,
    /// All the buckets of lower index are empty (`keep` path).
    first: usize,
    /// Buckets that became trees (rare).
    trees: Vec<TreeBin>,
    keep: bool,
}

impl Default for JSet {
    fn default() -> Self {
        JSet::with_keep(crate::opt::jset_keep())
    }
}

impl JSet {
    /// `keep`: path chosen explicitly (equivalence tests).
    pub fn with_keep(keep: bool) -> Self {
        JSet { buckets: Vec::new(), cap: 0, len: 0, first: 0, trees: Vec::new(), keep }
    }

    pub fn clear(&mut self) {
        if self.keep {
            for b in &mut self.buckets[..self.cap] {
                b.clear();
            }
            self.cap = 0;
        } else {
            self.buckets.truncate(16);
            for b in &mut self.buckets {
                b.clear();
            }
            self.cap = self.buckets.len();
        }
        self.trees.clear();
        self.first = 0;
        self.len = 0;
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn contains(&self, p: [i32; 3]) -> bool {
        if self.cap == 0 {
            return false;
        }
        let i = jhash(p) as usize & (self.cap - 1);
        self.buckets[i].contains(&p)
    }

    /// Buckets currently a tree (measurement, tests).
    pub fn tree_bins(&self) -> usize {
        self.trees.len()
    }

    fn tree_at(&self, i: usize) -> Option<usize> {
        if self.trees.is_empty() { None } else { self.trees.iter().position(|t| t.bucket == i) }
    }

    pub fn insert(&mut self, p: [i32; 3]) -> bool {
        if self.cap < 16 {
            if self.buckets.len() < 16 {
                self.buckets.resize_with(16, Vec::new);
            }
            self.cap = 16;
        }
        let cap = self.cap;
        let i = jhash(p) as usize & (cap - 1);
        if self.buckets[i].contains(&p) {
            return false;
        }
        self.len += 1;
        self.first = self.first.min(i);
        if let Some(t) = self.tree_at(i) {
            self.trees[t].insert(&mut self.buckets[i], p);
        } else {
            self.buckets[i].push(p);
            // List of more than 8 elements: table doubled under 64 buckets, tree beyond; then the
            // usual fill threshold.
            if self.buckets[i].len() > TREEIFY {
                if cap < MIN_TREE_CAP {
                    self.grow();
                } else {
                    let t = TreeBin::build(i, &mut self.buckets[i]);
                    self.trees.push(t);
                }
            }
        }
        if self.len > self.cap * 3 / 4 {
            self.grow();
        }
        true
    }

    /// Doubles the table (relative order kept in each bucket, like Java), then splits the trees.
    fn grow(&mut self) {
        let old = self.cap;
        let ncap = old * 2;
        if self.keep {
            if self.buckets.len() < ncap {
                self.buckets.resize_with(ncap, Vec::new);
            }
            let (lo, hi) = self.buckets[..ncap].split_at_mut(old);
            for (a, b) in lo.iter_mut().zip(hi.iter_mut()) {
                a.retain(|q| {
                    let up = jhash(*q) as usize & old != 0;
                    if up {
                        b.push(*q);
                    }
                    !up
                });
            }
        } else {
            let prev = std::mem::take(&mut self.buckets);
            self.buckets.resize_with(ncap, Vec::new);
            for b in &prev {
                for &q in b {
                    let j = jhash(q) as usize & (ncap - 1);
                    self.buckets[j].push(q);
                }
            }
        }
        self.cap = ncap;
        self.first = 0;
        if !self.trees.is_empty() {
            self.split_trees(old);
        }
    }

    /// Trees after doubling: each half becomes a list again at 6 elements or fewer, otherwise tree
    /// rebuilt, or the original tree kept as is if the other half is empty.
    fn split_trees(&mut self, bit: usize) {
        let old = std::mem::take(&mut self.trees);
        for mut t in old {
            let i = t.bucket;
            let (lc, hc) = (self.buckets[i].len(), self.buckets[i + bit].len());
            if lc > UNTREEIFY {
                if hc > 0 {
                    let nt = TreeBin::build(i, &mut self.buckets[i]);
                    self.trees.push(nt);
                } else {
                    self.trees.push(t.clone());
                }
            }
            if hc > UNTREEIFY {
                if lc > 0 {
                    let nt = TreeBin::build(i + bit, &mut self.buckets[i + bit]);
                    self.trees.push(nt);
                } else {
                    t.bucket = i + bit;
                    self.trees.push(t);
                }
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = [i32; 3]> + '_ {
        self.buckets[..self.cap].iter().flat_map(|b| b.iter().copied())
    }

    /// Removes and returns the first element of the iteration.
    pub fn pop_first(&mut self) -> Option<[i32; 3]> {
        let start = if self.keep { self.first } else { 0 };
        let i = (start..self.cap).find(|&i| !self.buckets[i].is_empty())?;
        self.first = i;
        self.len -= 1;
        let k = self.buckets[i].remove(0);
        if let Some(t) = self.tree_at(i) {
            if self.buckets[i].is_empty() {
                self.trees.swap_remove(t);
            } else {
                self.trees[t].remove(k);
            }
        }
        Some(k)
    }

    /// Copy stably sorted by y (context of the decorators).
    pub fn sorted_by_y(&self, out: &mut Vec<[i32; 3]>) {
        out.clear();
        out.extend(self.iter());
        out.sort_by_key(|p| p[1]);
    }
}
