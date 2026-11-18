//! FROZEN copy (22/10/2026) of the bounded relight after changes (`relight_sections`) and of the
//! full light of a chunk (`light_chunk`), as before any optimisation of
//! series 2. It serves as the expected result for the benches (`benches/`) and the equivalence tests:
//! an optimised path must return exactly the same sheets and the same touched sections.
//! Never optimise it nor "fix" it; an intended behaviour change of the engine is
//! carried over here by hand, in the same commit, with its reason.
//!
//! It reads only the four raw arrays from the tables (emission, attenuation, shape, masks)
//! and shares no function with the working path.

use std::sync::Arc;

use rc_world::{LIGHT_BYTES, LightLayer};

use crate::{Columns, LightGrid, LightTables, Touched};

const FULL: [u64; 4] = [u64::MAX; 4];
const OPPOSITE: [usize; 6] = [1, 0, 3, 2, 5, 4];
const STEP: [[i32; 3]; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];
const GROUP: i32 = 8;

fn face(t: &LightTables, s: u32) -> u32 {
    t.face.get(s as usize).copied().unwrap_or(0)
}

fn opacity(t: &LightTables, s: u32) -> u8 {
    t.dampening.get(s as usize).copied().unwrap_or(15).max(1)
}

fn damp(t: &LightTables, s: u32) -> u8 {
    t.dampening.get(s as usize).copied().unwrap_or(0)
}

fn emission(t: &LightTables, s: u32) -> u8 {
    t.emission.get(s as usize).copied().unwrap_or(0)
}

fn occluded(t: &LightTables, a: u32, b: u32, dir: usize) -> bool {
    let (fa, fb) = (face(t, a), face(t, b));
    if fa == 0 && fb == 0 {
        return false;
    }
    let (ma, mb) = (t.masks[fa as usize][dir], t.masks[fb as usize][OPPOSITE[dir]]);
    (0..4).all(|i| ma[i] | mb[i] == FULL[i])
}

fn nibble(l: &[u8; LIGHT_BYTES], x: usize, y: usize, z: usize) -> u8 {
    let i = (y << 8) | (z << 4) | x;
    (l[i >> 1] >> ((i & 1) * 4)) & 15
}

fn set_nibble(l: &mut [u8; LIGHT_BYTES], x: usize, y: usize, z: usize, v: u8) {
    let i = (y << 8) | (z << 4) | x;
    let sh = (i & 1) * 4;
    l[i >> 1] = (l[i >> 1] & !(15 << sh)) | (v << sh);
}

/// Reference of [`crate::relight_sections`] (without the counters).
pub fn relight_sections(
    grid: &mut impl LightGrid,
    height: i32,
    t: &LightTables,
    has_sky: bool,
    changes: &[[i32; 3]],
) -> Touched {
    let mut touched = Touched::new();
    type Group = ([i32; 3], [i32; 3], Vec<[i32; 3]>);
    let mut groups: Vec<Group> = Vec::new();
    for &c in changes {
        let near = |lo: &[i32; 3], hi: &[i32; 3]| (0..3).all(|a| c[a] >= lo[a] - GROUP && c[a] <= hi[a] + GROUP);
        match groups.iter_mut().find(|(lo, hi, _)| near(lo, hi)) {
            Some((lo, hi, members)) => {
                for a in 0..3 {
                    lo[a] = lo[a].min(c[a]);
                    hi[a] = hi[a].max(c[a]);
                }
                members.push(c);
            }
            None => groups.push((c, c, vec![c])),
        }
    }
    for (_, _, members) in &groups {
        solve(grid, height, t, members, false, &mut touched);
        if has_sky {
            solve(grid, height, t, members, true, &mut touched);
        }
    }
    touched
}

struct Zone {
    lo: [i32; 3],
    size: [i32; 3],
    state: Vec<u32>,
    data: Vec<bool>,
    value: Vec<u8>,
    inner: Vec<bool>,
}

impl Zone {
    fn index(&self, p: [i32; 3]) -> Option<usize> {
        let r = [p[0] - self.lo[0], p[1] - self.lo[1], p[2] - self.lo[2]];
        (0..3)
            .all(|a| (0..self.size[a]).contains(&r[a]))
            .then(|| ((r[1] * self.size[2] + r[2]) * self.size[0] + r[0]) as usize)
    }
}

fn state_at(grid: &impl LightGrid, x: i32, y: i32, z: i32, height: i32) -> u32 {
    if !(0..height).contains(&y) {
        return 0;
    }
    grid.column(x >> 4, z >> 4).map_or(0, |(sections, _, _)| {
        sections
            .get((y >> 4) as usize)
            .map_or(0, |s| s.blocks.get(rc_world::block_index((x & 15) as usize, (y & 15) as usize, (z & 15) as usize)))
    })
}

fn clear_above(grid: &impl LightGrid, t: &LightTables, x: i32, z: i32, from: i32, height: i32) -> bool {
    let Some((sections, _, _)) = grid.column(x >> 4, z >> 4) else { return true };
    let (lx, lz) = ((x & 15) as usize, (z & 15) as usize);
    let at = |y: i32| {
        if (0..height).contains(&y) {
            sections
                .get((y >> 4) as usize)
                .map_or(0, |s| s.blocks.get(rc_world::block_index(lx, (y & 15) as usize, lz)))
        } else {
            0
        }
    };
    let mut hi = at(height);
    for y in (from.max(-1)..height).rev() {
        let lo = at(y);
        if damp(t, lo) != 0 || occluded(t, hi, lo, 0) {
            return false;
        }
        hi = lo;
    }
    true
}

fn solve(grid: &mut impl LightGrid, height: i32, t: &LightTables, cs: &[[i32; 3]], sky: bool, touched: &mut Touched) {
    let (mut bmin, mut bmax) = (cs[0], cs[0]);
    for c in cs {
        for a in 0..3 {
            bmin[a] = bmin[a].min(c[a]);
            bmax[a] = bmax[a].max(c[a]);
        }
    }
    let (ylo, yhi) = if sky {
        let mut bottom = bmin[1];
        for c in cs {
            let mut b = c[1] - 1;
            while b >= 0 {
                let s = state_at(grid, c[0], b, c[2], height);
                let edge_ok = b == c[1] - 1 || !occluded(t, state_at(grid, c[0], b + 1, c[2], height), s, 0);
                if damp(t, s) != 0 || !edge_ok {
                    break;
                }
                b -= 1;
            }
            bottom = bottom.min(b + 1);
        }
        ((bottom - 15).max(0), (bmax[1] + 15).min(height - 1))
    } else {
        ((bmin[1] - 15).max(0), (bmax[1] + 15).min(height - 1))
    };
    let lo = [bmin[0] - 16, ylo - 1, bmin[2] - 16];
    let size = [bmax[0] - bmin[0] + 33, yhi - ylo + 3, bmax[2] - bmin[2] + 33];
    let n = (size[0] * size[1] * size[2]) as usize;
    let mut z = Zone { lo, size, state: vec![0; n], data: vec![false; n], value: vec![0; n], inner: vec![false; n] };
    for dz in 0..size[2] {
        for dx in 0..size[0] {
            let (x, wz) = (lo[0] + dx, lo[2] + dz);
            let col = grid.column(x >> 4, wz >> 4);
            let (lx, lz) = ((x & 15) as usize, (wz & 15) as usize);
            for dy in 0..size[1] {
                let y = lo[1] + dy;
                let i = ((dy * size[2] + dz) * size[0] + dx) as usize;
                let inner =
                    (1..size[0] - 1).contains(&dx) && (1..size[2] - 1).contains(&dz) && (1..size[1] - 1).contains(&dy);
                z.inner[i] = inner;
                let Some((sections, sky_layers, block_layers)) = col else { continue };
                if !(0..height).contains(&y) {
                    z.value[i] = if sky && y >= height { 15 } else { 0 };
                    continue;
                }
                z.state[i] = sections
                    .get((y >> 4) as usize)
                    .map_or(0, |s| s.blocks.get(rc_world::block_index(lx, (y & 15) as usize, lz)));
                let layers = if sky { sky_layers } else { block_layers };
                if let Some(Some(l)) = layers.get(((y >> 4) + 1) as usize) {
                    z.data[i] = true;
                    z.value[i] = nibble(l, lx, (y & 15) as usize, lz);
                }
            }
        }
    }
    let mut init = vec![0u8; n];
    let top_margin = lo[1] + size[1] - 1;
    for dz in 1..size[2] - 1 {
        for dx in 1..size[0] - 1 {
            let clear = || clear_above(&*grid, t, lo[0] + dx, lo[2] + dz, top_margin, height);
            let source_from = if sky && clear() { zone_source_height(&z, t, dx, dz, height) } else { i32::MAX };
            for dy in 1..size[1] - 1 {
                let i = ((dy * size[2] + dz) * size[0] + dx) as usize;
                if !z.data[i] {
                    continue;
                }
                init[i] = if sky { if lo[1] + dy >= source_from { 15 } else { 0 } } else { emission(t, z.state[i]) };
            }
        }
    }
    let mut level = vec![0u8; n];
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); 16];
    for i in 0..n {
        let v = if z.inner[i] {
            init[i]
        } else if z.data[i] || !sky || z.value[i] > 0 {
            z.value[i]
        } else {
            0
        };
        level[i] = v;
        if v > 1 && (z.inner[i] || z.data[i] || z.value[i] == 15) {
            buckets[v as usize].push(i);
        }
    }
    for l in (2..16usize).rev() {
        while let Some(i) = buckets[l].pop() {
            if level[i] as usize != l {
                continue;
            }
            let p = [
                lo[0] + (i as i32 % size[0]),
                lo[1] + (i as i32 / (size[0] * size[2])),
                lo[2] + ((i as i32 / size[0]) % size[2]),
            ];
            for (dir, s) in STEP.iter().enumerate() {
                let q = [p[0] + s[0], p[1] + s[1], p[2] + s[2]];
                let Some(j) = z.index(q) else { continue };
                if !z.inner[j] || !z.data[j] || occluded(t, z.state[i], z.state[j], dir) {
                    continue;
                }
                let nv = l as i32 - i32::from(opacity(t, z.state[j]));
                if nv > i32::from(level[j]) {
                    level[j] = nv as u8;
                    buckets[nv as usize].push(j);
                }
            }
        }
    }
    for dz in 1..size[2] - 1 {
        for dx in 1..size[0] - 1 {
            let (x, wz) = (lo[0] + dx, lo[2] + dz);
            let (lx, lz) = ((x & 15) as usize, (wz & 15) as usize);
            let changed: Vec<(i32, u8)> = (1..size[1] - 1)
                .filter_map(|dy| {
                    let i = ((dy * size[2] + dz) * size[0] + dx) as usize;
                    (z.data[i] && level[i] != z.value[i]).then_some((lo[1] + dy, level[i]))
                })
                .collect();
            if changed.is_empty() {
                continue;
            }
            let Some(layers) = grid.layers_mut(x >> 4, wz >> 4, sky) else { continue };
            let flat = rc_world::edge::ANY
                | rc_world::edge::bits(lx, rc_world::edge::X0)
                | rc_world::edge::bits(lz, rc_world::edge::Z0);
            let mut run: Option<(i32, u8)> = None;
            for (y, v) in changed {
                if let Some(Some(l)) = layers.get_mut(((y >> 4) + 1) as usize) {
                    set_nibble(Arc::make_mut(l), lx, (y & 15) as usize, lz, v);
                }
                let bits = flat | rc_world::edge::bits((y & 15) as usize, rc_world::edge::Y0);
                match &mut run {
                    Some((s, acc)) if *s == y >> 4 => *acc |= bits,
                    _ => {
                        if let Some((s, acc)) = run.replace((y >> 4, bits)) {
                            *touched.entry((x >> 4, s, wz >> 4)).or_insert(0) |= acc;
                        }
                    }
                }
            }
            if let Some((s, acc)) = run {
                *touched.entry((x >> 4, s, wz >> 4)).or_insert(0) |= acc;
            }
        }
    }
}

fn zone_source_height(z: &Zone, t: &LightTables, dx: i32, dz: i32, height: i32) -> i32 {
    let at = |dy: i32| z.state[((dy * z.size[2] + dz) * z.size[0] + dx) as usize];
    let mut top = z.size[1] - 2;
    while top >= 1 && z.lo[1] + top >= height {
        top -= 1;
    }
    for dy in (1..=top).rev() {
        let (hi, lo) = (at(dy + 1), at(dy));
        if damp(t, lo) != 0 || occluded(t, hi, lo, 0) {
            return z.lo[1] + dy + 1;
        }
    }
    i32::MIN
}

// ---- Full light of a chunk (copy of `full.rs`) ----

const W: i32 = 48;

struct Grid<'a> {
    cols: Columns<'a>,
    height: i32,
}

impl Grid<'_> {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        let c = self.cols[((z >> 4) * 3 + (x >> 4)) as usize]?;
        Some(c[((y << 8) | ((z & 15) << 4) | (x & 15)) as usize])
    }

    fn index(&self, x: i32, y: i32, z: i32) -> usize {
        ((y * W + z) * W + x) as usize
    }
}

/// Reference of [`crate::light_chunk`] (without the counters).
pub fn light_chunk(t: &LightTables, cols: Columns, height: i32, has_sky: bool) -> (Vec<LightLayer>, Vec<LightLayer>) {
    let g = Grid { cols, height };
    let n = (W * W * height) as usize;
    let sections = (height / 16) as usize;
    let mut level = vec![0u8; n];
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); 16];
    for z in 0..W {
        for x in 0..W {
            if g.cols[((z >> 4) * 3 + (x >> 4)) as usize].is_none() {
                continue;
            }
            for y in 0..height {
                let s = g.state(x, y, z).unwrap_or(0);
                let e = emission(t, s);
                if e > 0 {
                    let i = g.index(x, y, z);
                    level[i] = e;
                    buckets[e as usize].push(i as u32);
                }
            }
        }
    }
    propagate(&g, t, &mut level, &mut buckets);
    let block = layers(&level, sections, |v| v.iter().all(|&b| b == 0), false);
    if !has_sky {
        return (vec![None; sections + 2], block);
    }
    level.fill(0);
    let mut src = vec![i32::MAX; (W * W) as usize];
    for z in 0..W {
        for x in 0..W {
            if g.cols[((z >> 4) * 3 + (x >> 4)) as usize].is_some() {
                src[(z * W + x) as usize] = column_source_height(&g, t, x, z).max(0);
            }
        }
    }
    for z in 0..W {
        for x in 0..W {
            let from = src[(z * W + x) as usize];
            if from == i32::MAX {
                continue;
            }
            for y in from..height {
                level[g.index(x, y, z)] = 15;
            }
            let mut top = from;
            for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let (nx, nz) = (x + dx, z + dz);
                if (0..W).contains(&nx) && (0..W).contains(&nz) {
                    let o = src[(nz * W + nx) as usize];
                    if o != i32::MAX {
                        top = top.max(o);
                    }
                }
            }
            for y in from..=top.min(height - 1) {
                buckets[15].push(g.index(x, y, z) as u32);
            }
        }
    }
    propagate(&g, t, &mut level, &mut buckets);
    let sky = layers(&level, sections, |v| v.iter().all(|&b| b == 0xff), true);
    (sky, block)
}

fn column_source_height(g: &Grid, t: &LightTables, x: i32, z: i32) -> i32 {
    let mut hi = 0u32;
    for y in (0..g.height).rev() {
        let lo = g.state(x, y, z).unwrap_or(0);
        if damp(t, lo) != 0 || occluded(t, hi, lo, 0) {
            return y + 1;
        }
        hi = lo;
    }
    i32::MIN
}

fn propagate(g: &Grid, t: &LightTables, level: &mut [u8], buckets: &mut [Vec<u32>]) {
    for l in (2..16usize).rev() {
        while let Some(i) = buckets[l].pop() {
            let i = i as usize;
            if level[i] as usize != l {
                continue;
            }
            let ii = i as i32;
            let (x, z, y) = (ii % W, (ii / W) % W, ii / (W * W));
            let Some(si) = g.state(x, y, z) else { continue };
            for (dir, s) in STEP.iter().enumerate() {
                let (qx, qy, qz) = (x + s[0], y + s[1], z + s[2]);
                if !(0..W).contains(&qx) || !(0..W).contains(&qz) || !(0..g.height).contains(&qy) {
                    continue;
                }
                let Some(sj) = g.state(qx, qy, qz) else { continue };
                if occluded(t, si, sj, dir) {
                    continue;
                }
                let nv = l as i32 - i32::from(opacity(t, sj));
                let j = g.index(qx, qy, qz);
                if nv > i32::from(level[j]) {
                    level[j] = nv as u8;
                    buckets[nv as usize].push(j as u32);
                }
            }
        }
    }
}

fn layers(level: &[u8], sections: usize, empty: impl Fn(&[u8]) -> bool, trailing: bool) -> Vec<LightLayer> {
    let mut out: Vec<LightLayer> = vec![None; sections + 2];
    let mut keep = !trailing;
    for s in (0..sections).rev() {
        let mut a = [0u8; LIGHT_BYTES];
        for ly in 0..16usize {
            let y = (s * 16 + ly) as i32;
            for z in 0..16usize {
                for x in 0..16usize {
                    let v = level[((y * W + z as i32 + 16) * W + x as i32 + 16) as usize];
                    let i = (ly << 8) | (z << 4) | x;
                    a[i >> 1] |= v << ((i & 1) * 4);
                }
            }
        }
        let is_empty = empty(&a);
        if trailing {
            if !is_empty {
                keep = true;
            }
            if keep {
                out[s + 1] = Some(Arc::new(a));
            }
        } else if !is_empty {
            out[s + 1] = Some(Arc::new(a));
        }
    }
    out
}
