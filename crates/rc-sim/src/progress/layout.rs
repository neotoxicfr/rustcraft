//! Positions of advancements in their tab: "Walker" tree in three passes, like the game
//! (only displayed advancements count; a hidden advancement hands over to its children).

use super::Tree;

struct N {
    adv: usize,
    parent: Option<usize>,
    prev: Option<usize>,
    child_index: i32,
    children: Vec<usize>,
    ancestor: usize,
    thread: Option<usize>,
    x: i32,
    y: f32,
    modv: f32,
    change: f32,
    shift: f32,
}

struct L<'a> {
    t: &'a Tree,
    n: Vec<N>,
}

impl L<'_> {
    fn node(&mut self, adv: usize, parent: Option<usize>, prev: Option<usize>, child_index: i32, depth: i32) -> usize {
        let i = self.n.len();
        self.n.push(N {
            adv,
            parent,
            prev,
            child_index,
            children: Vec::new(),
            ancestor: i,
            thread: None,
            x: depth,
            y: -1.0,
            modv: 0.0,
            change: 0.0,
            shift: 0.0,
        });
        let mut previous = None;
        for &c in &self.t.list[adv].children {
            previous = self.add_child(i, c, previous);
        }
        i
    }

    fn add_child(&mut self, this: usize, adv: usize, mut previous: Option<usize>) -> Option<usize> {
        if self.t.list[adv].display.is_some() {
            let idx = self.n[this].children.len() as i32 + 1;
            let x = self.n[this].x + 1;
            let c = self.node(adv, Some(this), previous, idx, x);
            self.n[this].children.push(c);
            previous = Some(c);
        } else {
            for &g in &self.t.list[adv].children {
                previous = self.add_child(this, g, previous);
            }
        }
        previous
    }

    fn first_walk(&mut self, v: usize) {
        if self.n[v].children.is_empty() {
            self.n[v].y = match self.n[v].prev {
                Some(p) => self.n[p].y + 1.0,
                None => 0.0,
            };
        } else {
            let mut default_ancestor: Option<usize> = None;
            let children = self.n[v].children.clone();
            for &c in &children {
                self.first_walk(c);
                let da = default_ancestor.unwrap_or(c);
                default_ancestor = Some(self.apportion(c, da));
            }
            self.execute_shifts(v);
            let first = self.n[children[0]].y;
            let last = self.n[*children.last().unwrap_or(&children[0])].y;
            let mid = (first + last) / 2.0;
            match self.n[v].prev {
                Some(p) => {
                    self.n[v].y = self.n[p].y + 1.0;
                    self.n[v].modv = self.n[v].y - mid;
                }
                None => self.n[v].y = mid,
            }
        }
    }

    fn second_walk(&mut self, v: usize, mod_sum: f32, depth: i32, mut min: f32) -> f32 {
        self.n[v].y += mod_sum;
        self.n[v].x = depth;
        if self.n[v].y < min {
            min = self.n[v].y;
        }
        let m = self.n[v].modv;
        for c in self.n[v].children.clone() {
            min = self.second_walk(c, mod_sum + m, depth + 1, min);
        }
        min
    }

    fn third_walk(&mut self, v: usize, offset: f32) {
        self.n[v].y += offset;
        for c in self.n[v].children.clone() {
            self.third_walk(c, offset);
        }
    }

    fn execute_shifts(&mut self, v: usize) {
        let mut shift = 0.0f32;
        let mut change = 0.0f32;
        let children = self.n[v].children.clone();
        for &c in children.iter().rev() {
            self.n[c].y += shift;
            self.n[c].modv += shift;
            change += self.n[c].change;
            shift += self.n[c].shift + change;
        }
    }

    fn prev_or_thread(&self, v: usize) -> Option<usize> {
        self.n[v].thread.or_else(|| self.n[v].children.first().copied())
    }

    fn next_or_thread(&self, v: usize) -> Option<usize> {
        self.n[v].thread.or_else(|| self.n[v].children.last().copied())
    }

    fn apportion(&mut self, this: usize, mut default_ancestor: usize) -> usize {
        let Some(prev) = self.n[this].prev else { return default_ancestor };
        let parent = self.n[this].parent.unwrap_or(this);
        let mut vir = this;
        let mut vor = this;
        let mut vil = prev;
        let mut vol = self.n[parent].children[0];
        let mut sir = self.n[this].modv;
        let mut sor = self.n[this].modv;
        let mut sil = self.n[vil].modv;
        let mut sol = self.n[vol].modv;
        while let (Some(nvil), Some(nvir)) = (self.next_or_thread(vil), self.prev_or_thread(vir)) {
            vil = nvil;
            vir = nvir;
            vol = self.prev_or_thread(vol).unwrap_or(vol);
            vor = self.next_or_thread(vor).unwrap_or(vor);
            self.n[vor].ancestor = this;
            let shift = self.n[vil].y + sil - (self.n[vir].y + sir) + 1.0;
            if shift > 0.0 {
                let anc = self.get_ancestor(vil, this, default_ancestor);
                self.move_subtree(anc, this, shift);
                sir += shift;
                sor += shift;
            }
            sil += self.n[vil].modv;
            sir += self.n[vir].modv;
            sol += self.n[vol].modv;
            sor += self.n[vor].modv;
        }
        if let Some(nt) = self.next_or_thread(vil)
            && self.next_or_thread(vor).is_none()
        {
            self.n[vor].thread = Some(nt);
            self.n[vor].modv += sil - sor;
        } else {
            if let Some(pt) = self.prev_or_thread(vir)
                && self.prev_or_thread(vol).is_none()
            {
                self.n[vol].thread = Some(pt);
                self.n[vol].modv += sir - sol;
            }
            default_ancestor = this;
        }
        default_ancestor
    }

    fn move_subtree(&mut self, this: usize, right: usize, shift: f32) {
        let subtrees = (self.n[right].child_index - self.n[this].child_index) as f32;
        if subtrees != 0.0 {
            self.n[right].change -= shift / subtrees;
            self.n[this].change += shift / subtrees;
        }
        self.n[right].shift += shift;
        self.n[right].y += shift;
        self.n[right].modv += shift;
    }

    fn get_ancestor(&self, this: usize, other: usize, default_ancestor: usize) -> usize {
        let a = self.n[this].ancestor;
        let parent = self.n[other].parent;
        match parent {
            Some(p) if self.n[p].children.contains(&a) => a,
            _ => default_ancestor,
        }
    }
}

/// Positions of all trees whose root is displayed.
pub fn run(t: &mut Tree) {
    let roots: Vec<usize> =
        (0..t.list.len()).filter(|&i| t.list[i].parent.is_none() && t.list[i].display.is_some()).collect();
    let mut out: Vec<(usize, f32, f32)> = Vec::new();
    for r in roots {
        let mut l = L { t, n: Vec::new() };
        let root = l.node(r, None, None, 1, 0);
        l.first_walk(root);
        let y0 = l.n[root].y;
        let min = l.second_walk(root, 0.0, 0, y0);
        if min < 0.0 {
            l.third_walk(root, -min);
        }
        out.extend(l.n.iter().map(|n| (n.adv, n.x as f32, n.y)));
    }
    for (a, x, y) in out {
        t.list[a].x = x;
        t.list[a].y = y;
    }
}
