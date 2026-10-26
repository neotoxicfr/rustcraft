//! Portal geometry (S6), with no server access: Nether portal frame (search
//! for the interior, validity, number of portal blocks), largest rectangle around a
//! block, column search spiral, End portal frame pattern. Computations of game
//! 26.3 redone step by step (walk orders included) to return the same positions.

use crate::blocks::{Dir, P};

/// What the frame search sees of a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cell {
    /// Air or fire: possible interior.
    Empty,
    /// Nether portal block (interior too).
    Portal,
    /// Frame block (obsidian, tag `nether_portal_frame`).
    Frame,
    Other,
}

impl Cell {
    fn empty(self) -> bool {
        matches!(self, Cell::Empty | Cell::Portal)
    }

    fn frame(self) -> bool {
        self == Cell::Frame
    }
}

/// Horizontal axis of a portal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Axis {
    X,
    Z,
}

impl Axis {
    pub fn other(self) -> Axis {
        match self {
            Axis::X => Axis::Z,
            Axis::Z => Axis::X,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Axis::X => "x",
            Axis::Z => "z",
        }
    }

    pub fn parse(s: &str) -> Option<Axis> {
        match s {
            "x" => Some(Axis::X),
            "z" => Some(Axis::Z),
            _ => None,
        }
    }

    /// Component of `p` on this axis.
    pub fn of(self, p: P) -> i32 {
        match self {
            Axis::X => p[0],
            Axis::Z => p[2],
        }
    }

    pub fn of_f(self, p: [f64; 3]) -> f64 {
        match self {
            Axis::X => p[0],
            Axis::Z => p[2],
        }
    }
}

pub(crate) fn add(p: P, d: P, n: i32) -> P {
    [p[0] + d[0] * n, p[1] + d[1] * n, p[2] + d[2] * n]
}

/// Frame found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Shape {
    pub axis: Axis,
    /// Step toward the frame's "right" (west for the X axis, south for the Z axis).
    pub right: P,
    pub portal_blocks: i32,
    pub bottom_left: P,
    pub width: i32,
    pub height: i32,
}

impl Shape {
    pub fn is_valid(&self) -> bool {
        (2..=21).contains(&self.width) && (3..=21).contains(&self.height)
    }

    pub fn is_complete(&self) -> bool {
        self.is_valid() && self.portal_blocks == self.width * self.height
    }

    /// Interior cells in the game's order (x, then y, then z increasing).
    pub fn interior(&self) -> Vec<P> {
        let a = self.bottom_left;
        let b = add(add(a, [0, 1, 0], self.height - 1), self.right, self.width - 1);
        between_closed(a, b)
    }
}

/// All positions between two corners, x fastest, then y, then z.
pub(crate) fn between_closed(a: P, b: P) -> Vec<P> {
    let lo = [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])];
    let hi = [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])];
    let mut out = Vec::new();
    for z in lo[2]..=hi[2] {
        for y in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                out.push([x, y, z]);
            }
        }
    }
    out
}

/// Frame of a Nether portal around `pos`, on a given axis.
pub(crate) fn find_any_shape(at: &dyn Fn(P) -> Cell, min_y: i32, pos: P, axis: Axis) -> Shape {
    let right = match axis {
        Axis::X => Dir::West.step(),
        Axis::Z => Dir::South.step(),
    };
    let empty = |bl: P| Shape { axis, right, portal_blocks: 0, bottom_left: bl, width: 0, height: 0 };
    let Some(bl) = bottom_left(at, min_y, right, pos) else { return empty(pos) };
    let width = edge_distance(at, bl, right);
    let width = if (2..=21).contains(&width) { width } else { 0 };
    if width == 0 {
        return empty(bl);
    }
    let mut count = 0;
    let mut height = distance_to_top(at, bl, right, width, &mut count);
    if !((3..=21).contains(&height) && has_top_frame(at, bl, right, width, height)) {
        height = 0;
    }
    Shape { axis, right, portal_blocks: count, bottom_left: bl, width, height }
}

fn bottom_left(at: &dyn Fn(P) -> Cell, min_y: i32, right: P, mut pos: P) -> Option<P> {
    let lowest = min_y.max(pos[1] - 21);
    while pos[1] > lowest && at([pos[0], pos[1] - 1, pos[2]]).empty() {
        pos[1] -= 1;
    }
    let left = [-right[0], -right[1], -right[2]];
    let edge = edge_distance(at, pos, left) - 1;
    (edge >= 0).then(|| add(pos, left, edge))
}

/// Distance to the frame edge walking above the ledge.
fn edge_distance(at: &dyn Fn(P) -> Cell, pos: P, dir: P) -> i32 {
    for w in 0..=21 {
        let p = add(pos, dir, w);
        let c = at(p);
        if !c.empty() {
            if c.frame() {
                return w;
            }
            break;
        }
        if !at([p[0], p[1] - 1, p[2]]).frame() {
            break;
        }
    }
    0
}

fn distance_to_top(at: &dyn Fn(P) -> Cell, bl: P, right: P, width: i32, count: &mut i32) -> i32 {
    for h in 0..21 {
        let row = [bl[0], bl[1] + h, bl[2]];
        if !at(add(row, right, -1)).frame() || !at(add(row, right, width)).frame() {
            return h;
        }
        for i in 0..width {
            let c = at(add(row, right, i));
            if !c.empty() {
                return h;
            }
            if c == Cell::Portal {
                *count += 1;
            }
        }
    }
    21
}

fn has_top_frame(at: &dyn Fn(P) -> Cell, bl: P, right: P, width: i32, height: i32) -> bool {
    (0..width).all(|i| at(add([bl[0], bl[1] + height, bl[2]], right, i)).frame())
}

/// First valid frame on the preferred axis, then on the other.
pub(crate) fn find_shape(
    at: &dyn Fn(P) -> Cell,
    min_y: i32,
    pos: P,
    preferred: Axis,
    valid: impl Fn(&Shape) -> bool,
) -> Option<Shape> {
    let first = find_any_shape(at, min_y, pos, preferred);
    if valid(&first) {
        return Some(first);
    }
    let second = find_any_shape(at, min_y, pos, preferred.other());
    valid(&second).then_some(second)
}

/// Empty frame (no portal block at all) ready to be lit.
pub(crate) fn find_empty_shape(at: &dyn Fn(P) -> Cell, min_y: i32, pos: P, preferred: Axis) -> Option<Shape> {
    find_shape(at, min_y, pos, preferred, |s| s.is_valid() && s.portal_blocks == 0)
}

/// Rectangle found: minimum corner and sizes along the two axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rect {
    pub min: P,
    pub size1: i32,
    pub size2: i32,
}

fn axis_step(axis: u8) -> P {
    match axis {
        0 => [1, 0, 0],
        1 => [0, 1, 0],
        _ => [0, 0, 1],
    }
}

/// Largest rectangle of cells satisfying `test` around `center` in the (axis1, axis2) plane
/// (the search for the largest rectangle around a position). Axes: 0 = x, 1 = y, 2 = z.
pub(crate) fn largest_rectangle(
    center: P,
    axis1: u8,
    limit1: i32,
    axis2: u8,
    limit2: i32,
    test: &dyn Fn(P) -> bool,
) -> Rect {
    let (s1, s2) = (axis_step(axis1), axis_step(axis2));
    let limit = |from: P, d: P, n: i32| {
        let mut m = 0;
        let mut p = from;
        while m < n {
            p = add(p, d, 1);
            if !test(p) {
                break;
            }
            m += 1;
        }
        m
    };
    let neg = |d: P| [-d[0], -d[1], -d[2]];
    let neg1 = limit(center, neg(s1), limit1);
    let pos1 = limit(center, s1, limit1);
    let ci1 = neg1 as usize;
    let mut bounds = vec![(0i32, 0i32); ci1 + 1 + pos1 as usize];
    bounds[ci1] = (limit(center, neg(s2), limit2), limit(center, s2, limit2));
    let ci2 = bounds[ci1].0;
    for i in 1..=neg1 {
        let last = bounds[ci1 - (i as usize - 1)];
        let at = add(center, neg(s1), i);
        bounds[ci1 - i as usize] = (limit(at, neg(s2), last.0), limit(at, s2, last.1));
    }
    for i in 1..=pos1 {
        let last = bounds[ci1 + i as usize - 1];
        let at = add(center, s1, i);
        bounds[ci1 + i as usize] = (limit(at, neg(s2), last.0), limit(at, s2, last.1));
    }
    let (mut min1, mut min2, mut size1, mut size2) = (0i32, 0i32, 0i32, 0i32);
    let mut columns = vec![0i32; bounds.len()];
    for i2 in (0..=ci2).rev() {
        for (i1, b) in bounds.iter().enumerate() {
            let lo = ci2 - b.0;
            let hi = ci2 + b.1;
            columns[i1] = if i2 >= lo && i2 <= hi { hi + 1 - i2 } else { 0 };
        }
        let ((start, end), height) = max_rectangle(&columns);
        let n1 = 1 + end - start;
        if n1 * height > size1 * size2 {
            min1 = start;
            min2 = i2;
            size1 = n1;
            size2 = height;
        }
    }
    let min = add(add(center, s1, min1 - ci1 as i32), s2, min2 - ci2);
    Rect { min, size1, size2 }
}

/// Largest rectangle under a histogram: bounds and height.
pub(crate) fn max_rectangle(columns: &[i32]) -> ((i32, i32), i32) {
    let (mut max_start, mut max_end, mut max_h) = (0i32, 0i32, 0i32);
    let mut stack: Vec<usize> = vec![0];
    for column in 1..=columns.len() {
        let height = if column == columns.len() { 0 } else { columns[column] };
        while let Some(&top) = stack.last() {
            let sh = columns[top];
            if height >= sh {
                stack.push(column);
                break;
            }
            stack.pop();
            let start = stack.last().map_or(0, |&t| t + 1) as i32;
            if sh * (column as i32 - start) > max_h * (max_end - max_start) {
                max_end = column as i32;
                max_start = start;
                max_h = sh;
            }
        }
        if stack.is_empty() {
            stack.push(column);
        }
    }
    ((max_start, max_end - 1), max_h)
}

/// Columns in a spiral around `center`, first step `first`,
/// second `second`.
pub(crate) fn spiral(center: P, radius: i32, first: Dir, second: Dir) -> Vec<P> {
    let dirs = [first.step(), second.step(), first.opposite().step(), second.opposite().step()];
    let legs = 4 * radius;
    let mut leg = -1i32;
    let (mut leg_size, mut leg_index) = (0i32, 0i32);
    let mut last = add(center, second.step(), 1);
    let mut out = Vec::new();
    loop {
        let cur = add(last, dirs[((leg + 4) % 4) as usize], 1);
        last = cur;
        if leg_index >= leg_size {
            if leg >= legs {
                break;
            }
            leg += 1;
            leg_index = 0;
            leg_size = leg / 2 + 1;
        }
        leg_index += 1;
        out.push(cur);
    }
    out
}

/// Cell of the End frame pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Need {
    Any,
    /// Frame with an eye turned toward this direction.
    Eye(Dir),
}

const END_PATTERN: [&str; 5] = ["?vvv?", ">???<", ">???<", ">???<", "?^^^?"];

fn need(c: u8) -> Need {
    match c {
        b'^' => Need::Eye(Dir::South),
        b'>' => Need::Eye(Dir::West),
        b'v' => Need::Eye(Dir::North),
        b'<' => Need::Eye(Dir::East),
        _ => Need::Any,
    }
}

fn cross(a: P, b: P) -> P {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// End frame pattern around `pos`: "front top left" corner
/// of the first placement found. `eye(p)`: direction of the frame with an eye at `p`, otherwise `None`.
pub(crate) fn find_end_frame(pos: P, eye: &dyn Fn(P) -> Option<Dir>) -> Option<P> {
    let (w, h, d) = (5i32, 5i32, 1i32);
    let dist = w.max(h).max(d);
    for test in between_closed(pos, add(pos, [1, 1, 1], dist - 1)) {
        for fwd in Dir::ALL {
            for up in Dir::ALL {
                if up == fwd || up == fwd.opposite() {
                    continue;
                }
                let (f, u) = (fwd.step(), up.step());
                let r = cross(f, u);
                let ok = (0..w).all(|x| {
                    (0..h).all(|y| {
                        (0..d).all(|z| {
                            let p = [
                                test[0] - u[0] * y + r[0] * x + f[0] * z,
                                test[1] - u[1] * y + r[1] * x + f[1] * z,
                                test[2] - u[2] * y + r[2] * x + f[2] * z,
                            ];
                            match need(END_PATTERN[y as usize].as_bytes()[x as usize]) {
                                Need::Any => true,
                                Need::Eye(dir) => eye(p) == Some(dir),
                            }
                        })
                    })
                });
                if ok {
                    return Some(test);
                }
            }
        }
    }
    None
}

/// Aligned box: minimum corner, maximum corner.
pub(crate) type Bounds = ([f64; 3], [f64; 3]);

/// Closest point to `center` in the `allowed` zone minus the `blocked` boxes
/// (shape difference then closest point). The free zone is
/// split on the grid of the boxes' coordinates (two values within 1e-7 count as
/// one), regrouped into boxes like the game (strips in z, then widened in x, then in y);
/// at equal distance, the first box of that walk wins. `None` if nothing is free.
pub(crate) fn closest_free_point(allowed: Bounds, blocked: &[Bounds], center: [f64; 3]) -> Option<[f64; 3]> {
    let coords: [Vec<f64>; 3] = std::array::from_fn(|a| {
        let (lo, hi) = (allowed.0[a], allowed.1[a]);
        let mut v = vec![lo, hi];
        for b in blocked {
            v.extend([b.0[a], b.1[a]].into_iter().filter(|&c| c > lo && c < hi));
        }
        v.sort_by(f64::total_cmp);
        let mut out: Vec<f64> = Vec::with_capacity(v.len());
        for c in v {
            if out.last().is_none_or(|&last| c - last > 1.0e-7) {
                out.push(c);
            }
        }
        out
    });
    let n = [coords[0].len() - 1, coords[1].len() - 1, coords[2].len() - 1];
    let idx = |x: usize, y: usize, z: usize| (x * n[1] + y) * n[2] + z;
    let mut full = vec![false; n[0] * n[1] * n[2]];
    for x in 0..n[0] {
        for y in 0..n[1] {
            for z in 0..n[2] {
                let mid = [
                    (coords[0][x] + coords[0][x + 1]) / 2.0,
                    (coords[1][y] + coords[1][y + 1]) / 2.0,
                    (coords[2][z] + coords[2][z + 1]) / 2.0,
                ];
                full[idx(x, y, z)] = !blocked.iter().any(|b| (0..3).all(|a| mid[a] > b.0[a] && mid[a] < b.1[a]));
            }
        }
    }
    let strip_full = |full: &[bool], z0: usize, z1: usize, x: usize, y: usize| {
        x < n[0] && y < n[1] && (z0..z1).all(|z| full[idx(x, y, z)])
    };
    let mut best: Option<([f64; 3], f64)> = None;
    for y in 0..n[1] {
        for x in 0..n[0] {
            let mut start: Option<usize> = None;
            for z in 0..=n[2] {
                if z < n[2] && full[idx(x, y, z)] {
                    start.get_or_insert(z);
                    continue;
                }
                let Some(z0) = start.take() else { continue };
                let clear = |full: &mut [bool], cx: usize, cy: usize| {
                    for cz in z0..z {
                        full[idx(cx, cy, cz)] = false;
                    }
                };
                clear(&mut full, x, y);
                let mut end_x = x;
                while strip_full(&full, z0, z, end_x + 1, y) {
                    clear(&mut full, end_x + 1, y);
                    end_x += 1;
                }
                let mut end_y = y;
                while (x..=end_x).all(|cx| strip_full(&full, z0, z, cx, end_y + 1)) {
                    for cx in x..=end_x {
                        clear(&mut full, cx, end_y + 1);
                    }
                    end_y += 1;
                }
                let lo = [coords[0][x], coords[1][y], coords[2][z0]];
                let hi = [coords[0][end_x + 1], coords[1][end_y + 1], coords[2][z]];
                let p: [f64; 3] = std::array::from_fn(|a| center[a].clamp(lo[a], hi[a]));
                let d = (0..3).map(|a| (p[a] - center[a]).powi(2)).sum::<f64>();
                if best.is_none_or(|b| d < b.1) {
                    best = Some((p, d));
                }
            }
        }
    }
    best.map(|b| b.0)
}

/// Coordinate conversion between two dimensions.
pub(crate) fn teleport_scale(from_scale: f64, to_scale: f64) -> f64 {
    from_scale / to_scale
}

/// Relative position in the entry portal: right and
/// up in 0..1, forward in blocks.
pub(crate) fn relative_position(rect: Rect, axis: Axis, pos: [f64; 3], width: f64, height: f64) -> [f64; 3] {
    let w = f64::from(rect.size1) - width;
    let h = f64::from(rect.size2) - height;
    let min = rect.min;
    let right = if w > 0.0 {
        let start = f64::from(axis.of(min)) + width / 2.0;
        ((axis.of_f(pos) - start) / w).clamp(0.0, 1.0)
    } else {
        0.5
    };
    let up = if h > 0.0 { ((pos[1] - f64::from(min[1])) / h).clamp(0.0, 1.0) } else { 0.0 };
    let fwd_axis = axis.other();
    let forward = fwd_axis.of_f(pos) - (f64::from(fwd_axis.of(min)) + 0.5);
    [right, up, forward]
}

/// Arrival point in the exit portal: position, rotation
/// added to the yaw (0 or 90).
pub(crate) fn exit_position(
    rect: Rect,
    exit_axis: Axis,
    entry_axis: Axis,
    offset: [f64; 3],
    width: f64,
    height: f64,
) -> ([f64; 3], f32) {
    let bl = rect.min;
    let rotation = if entry_axis == exit_axis { 0.0 } else { 90.0 };
    let right = width / 2.0 + (f64::from(rect.size1) - width) * offset[0];
    let up = (f64::from(rect.size2) - height) * offset[1];
    let forward = 0.5 + offset[2];
    let x_aligned = exit_axis == Axis::X;
    let pos = [
        f64::from(bl[0]) + if x_aligned { right } else { forward },
        f64::from(bl[1]) + up,
        f64::from(bl[2]) + if x_aligned { forward } else { right },
    ];
    (pos, rotation)
}
