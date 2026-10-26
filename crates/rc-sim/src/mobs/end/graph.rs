//! Dragon flight graph (F15, `server-mobs.md` § 4.F15): 24 nodes around the origin of the fight
//! (12 on a circle of radius 60, 8 of radius 40 and 10 higher, 4 of radius 20), height at least
//! 73, neighbors fixed by bit masks; A* search on a binary heap that breaks ties like the
//! game's.

use super::super::mth::{cos, sin};

/// Neighbors of each node (bit i: node i is a neighbor).
const ADJACENCY: [u32; 24] = [
    6146, 8197, 8202, 16404, 32808, 32848, 65696, 131392, 131712, 263424, 526848, 525313, 1581057, 3166214, 2138120,
    6373424, 4358208, 12910976, 9044480, 9706496, 15216640, 13688832, 11763712, 8257536,
];

/// Graph node (block coordinates, search state).
#[derive(Debug, Clone, Copy)]
struct Node {
    p: [i32; 3],
    closed: bool,
    f: f32,
    g: f32,
    h: f32,
    came_from: Option<usize>,
    heap_idx: i32,
}

impl Node {
    fn new(p: [i32; 3]) -> Node {
        Node { p, closed: false, f: 0.0, g: 0.0, h: 0.0, came_from: None, heap_idx: -1 }
    }
}

/// Distance between two node positions (in `f32`).
fn dist(a: [i32; 3], b: [i32; 3]) -> f32 {
    let (xd, yd, zd) = ((b[0] - a[0]) as f32, (b[1] - a[1]) as f32, (b[2] - a[2]) as f32);
    (xd * xd + yd * yd + zd * zd).sqrt()
}

/// Squared distance.
fn dist_sqr(a: [i32; 3], b: [i32; 3]) -> f32 {
    let (xd, yd, zd) = ((b[0] - a[0]) as f32, (b[1] - a[1]) as f32, (b[2] - a[2]) as f32);
    xd * xd + yd * yd + zd * zd
}

/// Path to follow (`Path`): nodes in order, next index.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    pub nodes: Vec<[i32; 3]>,
    pub next: usize,
}

impl Path {
    pub fn is_done(&self) -> bool {
        self.next >= self.nodes.len()
    }

    pub fn advance(&mut self) {
        self.next += 1;
    }

    pub fn next_pos(&self) -> [i32; 3] {
        self.nodes[self.next]
    }
}

/// Dragon graph (nodes built on first use).
#[derive(Debug, Clone, Default)]
pub struct Graph {
    nodes: Vec<Node>,
    heap: Vec<usize>,
}

impl Graph {
    pub fn built(&self) -> bool {
        !self.nodes.is_empty()
    }

    /// Builds the 24 nodes; `height(x, z)`: "motion blocking, no leaves" height.
    pub fn build(&mut self, height: impl Fn(i32, i32) -> i32) {
        self.nodes.clear();
        for i in 0..24 {
            let mut y_adj = 5;
            let (r, step, m) = if i < 12 {
                (60.0f32, std::f32::consts::PI / 12.0, i)
            } else if i < 20 {
                y_adj += 10;
                (40.0, std::f32::consts::PI / 8.0, i - 12)
            } else {
                (20.0, std::f32::consts::PI / 4.0, i - 20)
            };
            let a = 2.0f32 * (-std::f32::consts::PI + step * m as f32);
            let x = (r * cos(a)).floor() as i32;
            let z = (r * sin(a)).floor() as i32;
            let y = 73.max(height(x, z) + y_adj);
            self.nodes.push(Node::new([x, y, z]));
        }
    }

    pub fn pos(&self, i: usize) -> [i32; 3] {
        self.nodes[i].p
    }

    /// Closest node to a position: only the inner ring
    /// (12..23) without living crystals; beyond 100 blocks, node 0.
    pub fn closest(&self, t: [f64; 3], inner_only: bool) -> usize {
        let cur = [t[0].floor() as i32, t[1].floor() as i32, t[2].floor() as i32];
        let mut best = 10_000.0f32;
        let mut idx = 0;
        let start = if inner_only { 12 } else { 0 };
        for i in start..self.nodes.len() {
            let d = dist_sqr(self.nodes[i].p, cur);
            if d < best {
                best = d;
                idx = i;
            }
        }
        idx
    }

    // ---------- Binary heap (game order) ----------

    fn heap_insert(&mut self, n: usize) {
        self.heap.push(n);
        let i = self.heap.len() - 1;
        self.nodes[n].heap_idx = i as i32;
        self.up_heap(i);
    }

    fn heap_pop(&mut self) -> usize {
        let popped = self.heap[0];
        let last = self.heap.pop().expect("heap");
        if !self.heap.is_empty() {
            self.heap[0] = last;
            self.nodes[last].heap_idx = 0;
            self.down_heap(0);
        }
        self.nodes[popped].heap_idx = -1;
        popped
    }

    fn change_cost(&mut self, n: usize, cost: f32) {
        let old = self.nodes[n].f;
        self.nodes[n].f = cost;
        let i = self.nodes[n].heap_idx as usize;
        if cost < old {
            self.up_heap(i);
        } else {
            self.down_heap(i);
        }
    }

    #[allow(clippy::neg_cmp_op_on_partial_ord)] // comparisons of the game's heap (NaN included)
    fn up_heap(&mut self, mut idx: usize) {
        let node = self.heap[idx];
        let cost = self.nodes[node].f;
        while idx > 0 {
            let parent_idx = (idx - 1) >> 1;
            let parent = self.heap[parent_idx];
            if !(cost < self.nodes[parent].f) {
                break;
            }
            self.heap[idx] = parent;
            self.nodes[parent].heap_idx = idx as i32;
            idx = parent_idx;
        }
        self.heap[idx] = node;
        self.nodes[node].heap_idx = idx as i32;
    }

    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    fn down_heap(&mut self, mut idx: usize) {
        let node = self.heap[idx];
        let cost = self.nodes[node].f;
        let size = self.heap.len();
        loop {
            let left = 1 + (idx << 1);
            let right = left + 1;
            if left >= size {
                break;
            }
            let lc = self.nodes[self.heap[left]].f;
            let rc = if right >= size { f32::INFINITY } else { self.nodes[self.heap[right]].f };
            let next = if lc < rc {
                if !(lc < cost) {
                    break;
                }
                left
            } else {
                if !(rc < cost) {
                    break;
                }
                right
            };
            let moved = self.heap[next];
            self.heap[idx] = moved;
            self.nodes[moved].heap_idx = idx as i32;
            idx = next;
        }
        self.heap[idx] = node;
        self.nodes[node].heap_idx = idx as i32;
    }

    /// Path from one node to another; `tail`: final node added after the target (or
    /// after the closest node reached). `inner_only`: nodes 0..11 excluded from the neighbors.
    pub fn find_path(&mut self, start: usize, end: usize, tail: Option<[i32; 3]>, inner_only: bool) -> Option<Path> {
        for n in &mut self.nodes {
            n.closed = false;
            n.f = 0.0;
            n.g = 0.0;
            n.h = 0.0;
            n.came_from = None;
            n.heap_idx = -1;
        }
        let to_p = self.nodes[end].p;
        let h = dist(self.nodes[start].p, to_p);
        let from = &mut self.nodes[start];
        from.g = 0.0;
        from.h = h;
        from.f = h;
        self.heap.clear();
        self.heap_insert(start);
        let mut closest = start;
        let min = if inner_only { 12 } else { 0 };
        while !self.heap.is_empty() {
            let open = self.heap_pop();
            if self.nodes[open].p == to_p {
                return Some(self.reconstruct(open, tail, Some(end)));
            }
            if dist(self.nodes[open].p, to_p) < dist(self.nodes[closest].p, to_p) {
                closest = open;
            }
            self.nodes[open].closed = true;
            for i in min..24 {
                if ADJACENCY[open] & (1 << i) == 0 {
                    continue;
                }
                if self.nodes[i].closed {
                    continue;
                }
                let tentative = self.nodes[open].g + dist(self.nodes[open].p, self.nodes[i].p);
                let in_open = self.nodes[i].heap_idx >= 0;
                if !in_open || tentative < self.nodes[i].g {
                    let hh = dist(self.nodes[i].p, to_p);
                    let n = &mut self.nodes[i];
                    n.came_from = Some(open);
                    n.g = tentative;
                    n.h = hh;
                    if in_open {
                        let c = n.g + n.h;
                        self.change_cost(i, c);
                    } else {
                        n.f = n.g + n.h;
                        self.heap_insert(i);
                    }
                }
            }
        }
        if closest == start {
            return None;
        }
        Some(self.reconstruct(closest, tail, None))
    }

    /// Chain of predecessors from `last` (then the final node if any).
    fn reconstruct(&self, last: usize, tail: Option<[i32; 3]>, _target: Option<usize>) -> Path {
        let mut nodes = vec![self.nodes[last].p];
        let mut cur = last;
        while let Some(c) = self.nodes[cur].came_from {
            nodes.insert(0, self.nodes[c].p);
            cur = c;
        }
        if let Some(f) = tail {
            nodes.push(f);
        }
        Path { nodes, next: 0 }
    }

    /// Node indices (tests).
    #[cfg(test)]
    pub fn positions(&self) -> Vec<[i32; 3]> {
        self.nodes.iter().map(|n| n.p).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn nodes_and_adjacency_symmetric() {
        let mut g = Graph::default();
        g.build(|_, _| 60);
        let p = g.positions();
        assert_eq!(p.len(), 24);
        // Outer ring: radius 60, height 73 (60 + 5 < 73).
        // Angle 2 × (−π): cosine 1, table sine 0.
        assert_eq!(p[0], [60, 73, 0]);
        assert_eq!(p[12][1], 75);
        for (i, m) in ADJACENCY.iter().enumerate() {
            for j in 0..24 {
                if m & (1 << j) != 0 {
                    assert!(ADJACENCY[j] & (1 << i) != 0, "{i} -> {j} without return");
                }
            }
        }
    }

    #[test]
    fn path_follows_ring() {
        let mut g = Graph::default();
        g.build(|_, _| 60);
        let path = g.find_path(0, 2, None, false).expect("path");
        assert_eq!(path.nodes.first(), Some(&g.pos(0)));
        assert_eq!(path.nodes.last(), Some(&g.pos(2)));
        // With final node: added last.
        let path = g.find_path(12, 14, Some([0, 70, 0]), true).expect("path");
        assert_eq!(path.nodes.last(), Some(&[0, 70, 0]));
        assert!(path.nodes.iter().all(|n| *n == [0, 70, 0] || g.positions()[12..].contains(n)));
    }
}
