//! Per-axis collisions and step-up (spec `movement.md` § 6.3).

use crate::{Aabb, World};

pub const EPS: f64 = 1.0e-7;

/// Collision boxes of the blocks that count in zone `b` (edge cell rules).
pub fn collect(w: &dyn World, b: &Aabb, out: &mut Vec<Aabb>) {
    collect_in(w, b, out, false);
}

/// Like `collect`, in the game's walk order (x then y then z): useful when the
/// first box found matters (auto-jump).
pub fn collect_ordered(w: &dyn World, b: &Aabb, out: &mut Vec<Aabb>) {
    collect_in(w, b, out, true);
}

/// Level of `RUSTCRAFT_OPT_ISFREE`, read once: 0 old path (`is_free` collects all the
/// boxes then queries the entities, new cell buffer on every collection); 1 `is_free` exits
/// at the first overlap and only queries the entities if blocks and border let through;
/// 2 (default): 1, plus the thread's cell buffer reused (collection and `is_free`).
pub fn is_free_mode() -> u8 {
    static M: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *M.get_or_init(|| match rc_data::env_str!("RUSTCRAFT_OPT_ISFREE").map(str::trim) {
        Some("0") => 0,
        Some("1") => 1,
        _ => 2,
    })
}

thread_local! {
    /// Buffer of the boxes of a cell, reused from one call to the next (level 2).
    static CELL: std::cell::RefCell<Vec<Aabb>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// `f` with a cell buffer: the thread's at level 2 (a new one if it is already borrowed by
/// a nested call), otherwise a new one.
pub(crate) fn with_cell<R>(mode: u8, f: impl FnOnce(&mut Vec<Aabb>) -> R) -> R {
    if mode >= 2 {
        CELL.with(|c| match c.try_borrow_mut() {
            Ok(mut v) => f(&mut v),
            Err(_) => f(&mut Vec::new()),
        })
    } else {
        f(&mut Vec::new())
    }
}

fn collect_in(w: &dyn World, b: &Aabb, out: &mut Vec<Aabb>, zyx: bool) {
    out.clear();
    with_cell(is_free_mode(), |cell| {
        scan(w, b, zyx, cell, |_, bx| {
            out.push(bx);
            false
        })
    });
}

/// Walks the collision boxes (placed in the world) that touch `b`, according to the edge
/// cell rules; `f` receives the cell and the box, and returns true to stop (the walk
/// then returns true).
pub(crate) fn scan(
    w: &dyn World,
    b: &Aabb,
    zyx: bool,
    cell: &mut Vec<Aabb>,
    mut f: impl FnMut([i32; 3], Aabb) -> bool,
) -> bool {
    let lo = [0, 1, 2].map(|a| (b.min[a] - EPS).floor() as i32 - 1);
    let hi = [0, 1, 2].map(|a| (b.max[a] + EPS).floor() as i32 + 1);
    // Outer loops: (y, z) or (z, y); x always the fastest.
    let (o, m) = if zyx { (2, 1) } else { (1, 2) };
    for a in lo[o]..=hi[o] {
        for c in lo[m]..=hi[m] {
            let (y, z) = if zyx { (c, a) } else { (a, c) };
            for x in lo[0]..=hi[0] {
                let edges = u8::from(x == lo[0] || x == hi[0])
                    + u8::from(y == lo[1] || y == hi[1])
                    + u8::from(z == lo[2] || z == hi[2]);
                cell.clear();
                // Corners ignored; two edges: only a block in piston movement.
                if edges >= 3 || (edges == 2 && !w.moving_collision(x, y, z, cell)) {
                    continue;
                }
                if edges < 2 {
                    let oversized = w.collision(x, y, z, cell);
                    if edges == 1 && !oversized {
                        continue;
                    }
                }
                for local in cell.iter() {
                    let bx = local.offset(f64::from(x), f64::from(y), f64::from(z));
                    if bx.intersects(b) && f([x, y, z], bx) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Distance allowed on axis `axis` for box `b` that wants to move by `d`.
pub fn clip_axis(boxes: &[Aabb], b: &Aabb, axis: usize, mut d: f64) -> f64 {
    let (o1, o2) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    for s in boxes {
        if d.abs() < EPS {
            return 0.0;
        }
        let overlap = |a: usize| s.max[a] - EPS > b.min[a] && s.min[a] + EPS < b.max[a];
        if !overlap(o1) || !overlap(o2) {
            continue;
        }
        if d > 0.0 && s.min[axis] >= b.max[axis] - EPS {
            d = d.min(s.min[axis] - b.max[axis]);
        } else if d < 0.0 && s.max[axis] <= b.min[axis] + EPS {
            d = d.max(s.max[axis] - b.min[axis]);
        }
    }
    if d.abs() < EPS { 0.0 } else { d }
}

/// Sweep Y, then X and Z (the larger first); returns the allowed movement.
pub fn sweep(boxes: &[Aabb], start: &Aabb, d: [f64; 3]) -> [f64; 3] {
    let mut m = [0.0f64; 3];
    let mut b = *start;
    let order = if d[0].abs() >= d[2].abs() { [1, 0, 2] } else { [1, 2, 0] };
    for axis in order {
        if d[axis] != 0.0 {
            let v = clip_axis(boxes, &b, axis, d[axis]);
            m[axis] = v;
            let mut o = [0.0; 3];
            o[axis] = v;
            b = b.offset(o[0], o[1], o[2]);
        }
    }
    m
}

/// Full movement with step-up (§ 6.3). `on_ground`: state before the movement.
pub fn resolve(w: &dyn World, start: &Aabb, d: [f64; 3], on_ground: bool, step: f64) -> [f64; 3] {
    // Shape of the world border, if the entity is close to it (`weather-border.md` § 4.5).
    let mut border = Vec::new();
    if let Some(b) = w.border() {
        b.collision(start, &mut border);
    }
    // Solid entities: searched once in the zone of the movement and the step.
    let mut ents = Vec::new();
    w.entity_colliders(&start.expand_towards(d).expand_towards([0.0, step.max(0.0), 0.0]), &mut ents);
    let mut boxes = Vec::new();
    collect(w, &start.expand_towards(d), &mut boxes);
    boxes.extend_from_slice(&border);
    boxes.extend_from_slice(&ents);
    let m = sweep(&boxes, start, d);
    let blocked_down = d[1] != m[1] && d[1] < 0.0;
    let horizontal = d[0] != m[0] || d[2] != m[2];
    if step <= 0.0 || !(blocked_down || on_ground) || !horizontal {
        return m;
    }
    let bs = if blocked_down { start.offset(0.0, m[1], 0.0) } else { *start };
    let mut zone = bs.expand_towards([d[0], step, d[2]]);
    if !blocked_down {
        zone = zone.expand_towards([0.0, f64::from(-1.0e-5f32), 0.0]);
    }
    collect(w, &zone, &mut boxes);
    boxes.extend_from_slice(&border);
    boxes.extend_from_slice(&ents);
    // Candidate heights: Y edges of the boxes relative to the bottom of Bs, in f32, increasing.
    let my = m[1] as f32;
    let mut heights: Vec<f32> = Vec::new();
    for s in &boxes {
        for y in [s.min[1], s.max[1]] {
            let c = (y - bs.min[1]) as f32;
            if c >= 0.0 && c != my && f64::from(c) <= step {
                heights.push(c);
            }
        }
    }
    heights.sort_by(f32::total_cmp);
    heights.dedup();
    let horiz = |v: [f64; 3]| v[0] * v[0] + v[2] * v[2];
    for c in heights {
        let t = sweep(&boxes, &bs, [d[0], f64::from(c), d[2]]);
        if horiz(t) > horiz(m) {
            let dy = start.min[1] - bs.min[1];
            return [t[0], t[1] - dy, t[2]];
        }
    }
    m
}

/// True if no collision box overlaps `b`.
pub fn is_free(w: &dyn World, b: &Aabb) -> bool {
    is_free_with(w, b, is_free_mode())
}

/// `is_free` at level `mode` of `RUSTCRAFT_OPT_ISFREE` (the proofs compare the levels).
pub fn is_free_with(w: &dyn World, b: &Aabb, mode: u8) -> bool {
    if mode == 0 {
        return is_free_old(w, b);
    }
    with_cell(mode, |cell| {
        // A box retained by the walk already overlaps `b` (same test as the old filter).
        if scan(w, b, false, cell, |_, _| true) {
            return false;
        }
        cell.clear();
        if let Some(border) = w.border() {
            border.collision(b, cell);
            if cell.iter().any(|s| s.intersects(b)) {
                return false;
            }
        }
        // Solid entities: the game searches in the box inflated by 1e-7, a simple contact blocks.
        cell.clear();
        w.entity_colliders(b, cell);
        cell.is_empty()
    })
}

/// Old `is_free` (`RUSTCRAFT_OPT_ISFREE=0`): all the boxes collected, entities always
/// queried.
fn is_free_old(w: &dyn World, b: &Aabb) -> bool {
    let mut boxes = Vec::new();
    collect(w, b, &mut boxes);
    if let Some(border) = w.border() {
        border.collision(b, &mut boxes);
    }
    let blocks = !boxes.iter().any(|s| (0..3).all(|a| s.max[a] > b.min[a] && s.min[a] < b.max[a]));
    // Solid entities: the game searches in the box inflated by 1e-7, a simple contact blocks.
    let mut ents = Vec::new();
    w.entity_colliders(b, &mut ents);
    blocks && ents.is_empty()
}
