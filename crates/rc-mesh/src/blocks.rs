//! Textured meshing: baked faces of the models (variant choice identical to the game),
//! removal of the hidden faces (opaque neighbour, or identical block for glasses, ices…),
//! tint computed per block (biome blended over 5 × 5 blocks, or fixed colour).
//! One 24-byte entry per face (`Face`): block position, index of the baked face (and
//! model offset, `offset_words`), tint, light and occlusion gray of the corners.

use std::sync::Arc;

use rc_assets::texture::{Colormaps, colormap};
use rc_assets::{BlockModels, geom::Dir};
use rc_data::{ClientState, VersionData, flags, tint};
use rc_world::{Column, SECTION_BLOCKS};

use crate::fluid::Blocks as _;
use crate::wire::MeshJob;

/// "None" tint (the shader leaves the texture colour).
pub const NO_TINT: u32 = 0xffff_ffff;
/// Default blend radius of the biome colours (2 → 5 × 5 blocks); setting: `MeshOptions::blend`.
pub const BIOME_BLEND: i32 = 2;

/// Per-state tables useful to the meshing, built once.
pub struct Tables {
    pub models: Arc<BlockModels>,
    pub air: Vec<bool>,
    /// Bit d (`Dir` order): face d of the state fully covers the side and hides the neighbour.
    pub covers: Vec<u8>,
    pub seed_offset: Vec<[i32; 3]>,
    pub block: Vec<u16>,
    /// Empty if the client jar was missing at preparation (then no tint nor removal between identical blocks).
    pub client: Vec<ClientState>,
    pub fluid: crate::fluid::FluidTables,
    /// Per state, union over all the variants: bit d = a face is removed against the neighbour d;
    /// `ALWAYS` = a face without removal (never hidden) or no known model.
    pub cull_dirs: Vec<u8>,
    /// Solid render states (full opaque cube), for the section visibility.
    pub solid: Vec<bool>,
    /// Random offset of the model per state: (kind, horizontal bound, vertical bound).
    pub offset: Vec<(u8, f32, f32)>,
    /// Offset class per state for the compact faces (`MeshOptions::offset_faces`):
    /// 1 + rank of the triple (kind, bounds) in `offset_classes`, 0 without offset or beyond
    /// `OFFSET_CLASSES` triples (explicit quad).
    pub offset_class: Vec<u8>,
    pub offset_classes: Vec<(u8, f32, f32)>,
    /// Group of each baked face: direction (`Dir` order) of its visible side if it is
    /// planar and aligned on an axis, otherwise `BUCKETS - 1` (always submitted).
    pub bucket: Vec<u8>,
    /// Light properties per state (`light::info`).
    pub light_info: Vec<u16>,
    /// Geometry of each baked face as seen by the lighting (`LightPaths::facetab`), built on a
    /// thread of its own: [`Tables::light_faces`].
    light_faces: Arc<std::sync::OnceLock<crate::light::FaceTables>>,
    /// States of the leaf blocks (`cutoutLeaves` setting).
    pub leaves: Vec<bool>,
    /// "Sealed" states: cube that covers its six sides, all its faces are removed against a
    /// covering neighbour, no fluid nor leaves rule. A section that contains only
    /// those and whose border covers its six faces has no face (exact shortcut).
    pub sealed: Vec<bool>,
    /// Face shapes, inclusion table and per-state properties of the per-block loop.
    pub cull: crate::cull::CullTables,
}

/// Groups of faces in the mesh: six aligned directions, then the others.
pub const BUCKETS: usize = 7;

/// Group of a face according to its visible side (`sign`: orientation of the cross product of the
/// corners that gives the front side for rendering, taken over the set of faces).
fn face_bucket(p: &[[f32; 3]; 4], sign: f32) -> u8 {
    let (a, b) = (
        std::array::from_fn::<f32, 3, _>(|i| p[1][i] - p[0][i]),
        std::array::from_fn::<f32, 3, _>(|i| p[2][i] - p[0][i]),
    );
    let n = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]].map(|v| v * sign);
    let len = n.iter().map(|v| v.abs()).fold(0.0, f32::max);
    let Some(axis) = (0..3).find(|&i| n[i].abs() == len) else { return (BUCKETS - 1) as u8 };
    let flat = (0..3).all(|i| i == axis || n[i].abs() <= len * 1e-6);
    let planar = p.iter().all(|c| (c[axis] - p[0][axis]).abs() <= 1e-6);
    if len == 0.0 || !flat || !planar {
        return (BUCKETS - 1) as u8;
    }
    let positive = n[axis] > 0.0;
    (match (axis, positive) {
        (1, false) => Dir::Down,
        (1, true) => Dir::Up,
        (2, false) => Dir::North,
        (2, true) => Dir::South,
        (0, false) => Dir::West,
        _ => Dir::East,
    }) as u8
}

#[cfg(test)]
#[test]
fn face_buckets_of_axis_and_tilted_quads() {
    // Top face of a cube, corners in the game's order; tilted: "others" group.
    let up = [[0.0, 1.0, 0.0], [0.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 1.0, 0.0]];
    let s = if face_bucket(&up, 1.0) == Dir::Up as u8 { 1.0 } else { -1.0 };
    assert_eq!(face_bucket(&up, s), Dir::Up as u8);
    assert_eq!(face_bucket(&up, -s), Dir::Down as u8);
    let tilted = [[0.0, 1.0, 0.0], [0.0, 0.5, 1.0], [1.0, 0.5, 1.0], [1.0, 1.0, 0.0]];
    assert_eq!(face_bucket(&tilted, s), (BUCKETS - 1) as u8);
    let bent = [[0.0, 1.0, 0.0], [0.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 0.9, 0.0]];
    assert_eq!(face_bucket(&bent, s), (BUCKETS - 1) as u8);
}

/// Groups of all the baked faces. The front side follows the order of the corners; its sign is
/// the one that puts the great majority of faces on the side of their declared direction.
fn face_buckets(models: &BlockModels) -> Vec<u8> {
    let agree = models.quads.iter().filter(|q| face_bucket(&q.pos, 1.0) == q.dir as u8).count();
    let sign = if agree * 2 >= models.quads.len() { 1.0 } else { -1.0 };
    models.quads.iter().map(|q| face_bucket(&q.pos, sign)).collect()
}

const ALWAYS: u8 = 0x80;

/// Block face in the mesh (see `BlockMesh::layers`).
pub type Face = [u32; 6];

/// Colour of a vertex like the game, in truncated integers: occlusion gray × directional
/// shading, then × tint / 255 (`docs/specs/lighting.md` § 2.4).
pub fn vertex_color(gray: u8, shade: f32, tint: u32) -> u32 {
    let c = ((f32::from(gray) * shade) as u32).min(255);
    if tint == NO_TINT {
        return c << 16 | c << 8 | c | 0xff00_0000;
    }
    let ch = |s: u32| c * ((tint >> s) & 0xff) / 255;
    ch(16) << 16 | ch(8) << 8 | ch(0) | 0xff00_0000
}
/// Border grid of a section (18³) and step of a neighbour per direction (`Dir` order).
const P3: usize = 18 * 18 * 18;
const PAD_STEP: [isize; 6] = [-324, 324, -18, 18, -1, 1];

/// Cell of the 18³ grid of the block of index `i` (y << 8 | z << 4 | x) of the section.
#[inline(always)]
const fn pad_index(i: usize) -> usize {
    i + 68 * (i >> 8) + 2 * ((i >> 4) & 15) + 343
}

/// Properties of the six neighbours (`Dir` order) of cell `c` of the grid.
#[inline(always)]
fn neighbor_props(cell_props: &[u32], c: usize) -> [u32; 6] {
    let p = cell_props;
    [p[c - 324], p[c + 324], p[c - 18], p[c + 18], p[c - 1], p[c + 1]]
}

/// Properties of the section interior read through its palette (a table of 2^bits entries);
/// `false` if the container has none (direct identifiers or palette too wide).
fn palette_props(
    cont: &rc_world::Container,
    ct: &crate::cull::CullTables,
    rule: usize,
    values: &[u32],
    cell_props: &mut [u32],
) -> bool {
    match cont {
        rc_world::Container::Single(v) => {
            let p = ct.prop(rule, *v);
            (0..SECTION_BLOCKS).for_each(|i| cell_props[pad_index(i)] = p);
            true
        }
        rc_world::Container::Packed { bits, palette: Some(pal), data } if (1..=8).contains(bits) => {
            let mut tab = [0u32; 256];
            for (k, e) in tab.iter_mut().enumerate().take(1 << bits) {
                *e = ct.prop(rule, pal.get(k).copied().unwrap_or(0));
            }
            let (b, vpl) = (*bits as usize, 64 / *bits as usize);
            let mask = (1u64 << b) - 1;
            let mut i = 0;
            'words: for &w in data.iter() {
                let mut w = w;
                for _ in 0..vpl {
                    if i == SECTION_BLOCKS {
                        break 'words;
                    }
                    cell_props[pad_index(i)] = tab[(w & mask) as usize];
                    w >>= b;
                    i += 1;
                }
            }
            // Cells beyond the words: values as unfolded.
            (i..SECTION_BLOCKS).for_each(|i| cell_props[pad_index(i)] = ct.prop(rule, values[i]));
            true
        }
        _ => false,
    }
}

/// Indices of the blocks of the section that can produce a face or a fluid (`cull::candidate`),
/// by rows of 16, in increasing order.
fn row_candidates(cell_props: &[u32], out: &mut Vec<u16>) {
    out.clear();
    for yz in 0..256usize {
        let base = pad_index(yz << 4);
        let mut m = 0u32;
        for x in 0..16 {
            let c = base + x;
            let p = cell_props[c];
            let sure = crate::cull::sure_hidden(p, neighbor_props(cell_props, c));
            m |= u32::from(crate::cull::candidate(p, sure)) << x;
        }
        while m != 0 {
            out.push(((yz << 4) as u32 | m.trailing_zeros()) as u16);
            m &= m - 1;
        }
    }
}

/// [`row_candidates`] computed on fixed 16-cell slices (the cell and its six neighbours), with
/// the surely hidden faces of each candidate kept in `sure`: the per-block loop reads them instead
/// of computing them again. AVX2 version when the processor has it (8 cells per instruction
/// instead of 4).
#[allow(unsafe_code)]
fn row_candidates_with_sure(cell_props: &[u32], candidates: &mut Vec<u16>, sure: &mut Vec<u8>) {
    #[cfg(target_arch = "x86_64")]
    {
        #[target_feature(enable = "avx2")]
        fn row_candidates_avx2(cell_props: &[u32], candidates: &mut Vec<u16>, sure: &mut Vec<u8>) {
            row_candidates_by_lanes(cell_props, candidates, sure);
        }
        if rc_data::cpu::has_avx2() {
            // SAFETY: AVX2 has just been detected on this processor.
            return unsafe { row_candidates_avx2(cell_props, candidates, sure) };
        }
    }
    row_candidates_by_lanes(cell_props, candidates, sure);
}

/// Body of [`row_candidates_with_sure`], compiled once per instruction set.
#[inline(always)]
fn row_candidates_by_lanes(cell_props: &[u32], candidates: &mut Vec<u16>, sure: &mut Vec<u8>) {
    candidates.clear();
    sure.clear();
    for row in 0..256usize {
        let cell = pad_index(row << 4);
        let lane = |step: isize| -> &[u32; 16] {
            let first = cell.wrapping_add_signed(step);
            cell_props[first..first + 16].try_into().expect("16 cells")
        };
        // Written out rather than `PAD_STEP.map(lane)`, which the compiler leaves as a call.
        let [down, up, north, south, west, east] = PAD_STEP;
        let neighbours = [lane(down), lane(up), lane(north), lane(south), lane(west), lane(east)];
        let (mut mask, row_sure) = candidate_row(lane(0), neighbours);
        while mask != 0 {
            let x = mask.trailing_zeros() as usize;
            candidates.push((row << 4 | x) as u16);
            sure.push(row_sure[x] as u8);
            mask &= mask - 1;
        }
    }
}

/// Candidate mask (bit x) and surely hidden faces of the 16 cells of a row, from the row and its
/// six neighbouring rows (`Dir` order). Same computation for every lane, no branch: the
/// compiler turns the loop into vector instructions.
#[inline(always)]
fn candidate_row(center: &[u32; 16], neighbours: [&[u32; 16]; 6]) -> (u32, [u32; 16]) {
    let mut sure = [0u32; 16];
    let mut mask = 0u32;
    for x in 0..16 {
        let s = crate::cull::sure_hidden(center[x], neighbours.map(|n| n[x]));
        sure[x] = s;
        mask |= crate::cull::candidate_bit(center[x], s) << x;
    }
    (mask, sure)
}

/// Bits of the solid render cells of 16 consecutive cells of the property grid.
#[inline(always)]
fn solid_row_bits(props: &[u32]) -> u64 {
    props.iter().enumerate().fold(0, |m, (x, &p)| m | u64::from(p & crate::cull::SOLID_RENDER_BIT != 0) << x)
}

/// Solid render mask of the section (bit i = block i) read from the interior of the property grid.
fn solid_mask_of_props(cell_props: &[u32]) -> [u64; 64] {
    let mut bits = [0u64; 64];
    for row in 0..256usize {
        let cell = pad_index(row << 4);
        // 16 cells per row, 4 rows per 64-bit word of the mask.
        bits[row >> 2] |= solid_row_bits(&cell_props[cell..cell + 16]) << ((row & 3) * 16);
    }
    bits
}

/// [`sealed_section`] read from the property grid: same test (sealed inside,
/// covering border on the section side).
fn sealed_props(cell_props: &[u32]) -> bool {
    use crate::cull::{HS, SEALED_BIT};
    if !(0..SECTION_BLOCKS).all(|i| cell_props[pad_index(i)] & SEALED_BIT != 0) {
        return false;
    }
    let covers = |i: usize, d: Dir| cell_props[i] >> (HS + d as u32) & 1 == 1;
    (1..17).all(|a| {
        (1..17).all(|b| {
            covers(a * 18 + b, Dir::Up)
                && covers(17 * 324 + a * 18 + b, Dir::Down)
                && covers(a * 324 + b, Dir::South)
                && covers(a * 324 + 17 * 18 + b, Dir::North)
                && covers(a * 324 + b * 18, Dir::East)
                && covers(a * 324 + b * 18 + 17, Dir::West)
        })
    })
}

/// Blocks of one (y, z) row of a section.
const ROW_BLOCKS: usize = 256;

/// States of a section of the centre column, as decoded for this meshing.
enum UnfoldedSection {
    /// Empty (air), or not read by this meshing (partial remesh, far from the remeshed sections).
    Absent,
    /// All the states (index y << 8 | z << 4 | x).
    Full(Box<[u32; SECTION_BLOCKS]>),
    /// Only the rows y = 0 then y = 15: neighbour of a remeshed section, outside the remesh,
    /// read only as the border of its neighbour (`CullPaths::border_rows_only`).
    BorderRows(Box<[u32; 2 * ROW_BLOCKS]>),
}

/// Row y = 0 or y = 15 of a section, seen as the bottom or top border of its neighbour.
#[derive(Clone, Copy)]
enum BorderRow<'a> {
    /// Below the bottom or above the top of the world.
    Outside,
    Air,
    States(&'a [u32; ROW_BLOCKS]),
}

impl BorderRow<'_> {
    /// Row `y` (0 or 15) of section `s` of the column (`None`: below the world).
    fn of(sections: &[UnfoldedSection], s: Option<usize>, y: usize) -> BorderRow<'_> {
        debug_assert!(y == 0 || y == 15, "border row {y}");
        match s.and_then(|s| sections.get(s)) {
            None => BorderRow::Outside,
            Some(UnfoldedSection::Absent) => BorderRow::Air,
            Some(UnfoldedSection::Full(v)) => BorderRow::States(&v.as_chunks::<ROW_BLOCKS>().0[y]),
            Some(UnfoldedSection::BorderRows(r)) => {
                BorderRow::States(&r.as_chunks::<ROW_BLOCKS>().0[usize::from(y == 15)])
            }
        }
    }

    /// State of block `xz` (z << 4 | x) of the row.
    #[inline]
    fn at(self, xz: usize) -> u32 {
        match self {
            BorderRow::Outside => OUTSIDE,
            BorderRow::Air => 0,
            BorderRow::States(r) => r[xz],
        }
    }
}

/// What the fill of a section's 18³ grids reads besides the section's own states.
struct GridFill<'a> {
    job: &'a MeshJob,
    sections: &'a [UnfoldedSection],
    height: i32,
    paths: crate::cull::CullPaths,
    cull_tables: &'a crate::cull::CullTables,
    /// Face removal rule (index of `CullTables::props`).
    rule: usize,
    /// The property grid is filled along with the states one.
    with_props: bool,
}

/// Fills the 18³ grids of section `si` (step x 1, z 18, y 324; `OUTSIDE` outside the available
/// columns): states, and properties when the grid is used. Returns the solid render mask of the
/// section when it is read from the properties (`CullPaths::solid_from_props`).
fn fill_section_grids(
    f: &GridFill<'_>,
    si: usize,
    values: &[u32; SECTION_BLOCKS],
    state_grid: &mut [u32],
    cell_props: &mut [u32],
) -> Option<[u64; 64]> {
    let paths = f.paths;
    // Interior through the palette: the interior cells then skip their per-state lookup.
    let interior_props_done = f.with_props
        && paths.palprops
        && palette_props(&f.job.center.sections[si].blocks, f.cull_tables, f.rule, &values[..], cell_props);
    let mut solid = (f.with_props && paths.solid_from_props).then_some([0u64; 64]);
    if paths.fill_rows {
        fill_interior_rows(f, values, state_grid, cell_props, interior_props_done, solid.as_mut());
        fill_bottom_and_top_rows(f, si, state_grid, cell_props);
        for pz in 0..18 {
            for px in 0..18 {
                if px == 0 || px == 17 || pz == 0 || pz == 17 {
                    fill_ring_stack(f, si, (px, pz), state_grid, cell_props);
                }
            }
        }
    } else {
        fill_by_stacks(f, si, values, state_grid, cell_props, interior_props_done);
        if let Some(s) = solid.as_mut() {
            *s = solid_mask_of_props(cell_props);
        }
    }
    solid
}

/// Interior of the grids by (y, z) rows: the 16 contiguous states of the section copied to 16
/// contiguous cells, their properties looked up in the same pass (and their solid render bits
/// gathered into `solid`).
fn fill_interior_rows(
    f: &GridFill<'_>,
    values: &[u32; SECTION_BLOCKS],
    state_grid: &mut [u32],
    cell_props: &mut [u32],
    props_done: bool,
    mut solid: Option<&mut [u64; 64]>,
) {
    let table = &f.cull_tables.props[f.rule][..];
    for (row, states) in values.as_chunks::<16>().0.iter().enumerate() {
        let cell = pad_index(row << 4);
        state_grid[cell..cell + 16].copy_from_slice(states);
        if !f.with_props {
            continue;
        }
        let props = &mut cell_props[cell..cell + 16];
        if !props_done {
            for (p, &s) in props.iter_mut().zip(states) {
                *p = crate::cull::state_prop(table, s);
            }
        }
        if let Some(solid) = solid.as_deref_mut() {
            // 16 cells per row, 4 rows per 64-bit word of the mask.
            solid[row >> 2] |= solid_row_bits(props) << ((row & 3) * 16);
        }
    }
}

/// Bottom and top layers of the grids inside x and z (y = -1 and 16): rows 15 and 0 of the
/// sections below and above, copied 16 cells at a time.
fn fill_bottom_and_top_rows(f: &GridFill<'_>, si: usize, state_grid: &mut [u32], cell_props: &mut [u32]) {
    let below = BorderRow::of(f.sections, si.checked_sub(1), 15);
    let above = BorderRow::of(f.sections, Some(si + 1), 0);
    for (grid_y, row) in [(0, below), (17, above)] {
        for z in 0..16 {
            let cell = grid_y * 324 + (z + 1) * 18 + 1;
            let states = &mut state_grid[cell..cell + 16];
            match row {
                BorderRow::Outside => states.fill(OUTSIDE),
                BorderRow::Air => states.fill(0),
                BorderRow::States(r) => states.copy_from_slice(&r[z * 16..z * 16 + 16]),
            }
            if f.with_props {
                for (p, &s) in cell_props[cell..cell + 16].iter_mut().zip(&*states) {
                    *p = f.cull_tables.prop(f.rule, s);
                }
            }
        }
    }
}

/// Old order of the fill: one vertical stack (step 324) per (x, z) of the grids.
fn fill_by_stacks(
    f: &GridFill<'_>,
    si: usize,
    values: &[u32; SECTION_BLOCKS],
    state_grid: &mut [u32],
    cell_props: &mut [u32],
    props_done: bool,
) {
    let below = BorderRow::of(f.sections, si.checked_sub(1), 15);
    let above = BorderRow::of(f.sections, Some(si + 1), 0);
    let (ct, rule) = (f.cull_tables, f.rule);
    for pz in 0..18usize {
        for px in 0..18usize {
            if !((1..17).contains(&px) && (1..17).contains(&pz)) {
                fill_ring_stack(f, si, (px, pz), state_grid, cell_props);
                continue;
            }
            let column_base = pz * 18 + px;
            let xz = ((pz - 1) << 4) | (px - 1);
            for y in 0..16 {
                state_grid[column_base + (y + 1) * 324] = values[(y << 8) | xz];
            }
            if f.with_props && !props_done {
                for y in 0..16 {
                    cell_props[column_base + (y + 1) * 324] = ct.prop(rule, values[(y << 8) | xz]);
                }
            }
            let top = column_base + 17 * 324;
            state_grid[column_base] = below.at(xz);
            state_grid[top] = above.at(xz);
            if f.with_props {
                cell_props[column_base] = ct.prop(rule, state_grid[column_base]);
                cell_props[top] = ct.prop(rule, state_grid[top]);
            }
        }
    }
}

/// Stack (px, pz) of the ring of the grids (x or z on the border), 18 cells from y = -1 to 16,
/// read from the neighbouring column.
fn fill_ring_stack(
    f: &GridFill<'_>,
    si: usize,
    (px, pz): (usize, usize),
    state_grid: &mut [u32],
    cell_props: &mut [u32],
) {
    let (x, z) = (px as i32 - 1, pz as i32 - 1);
    let column_base = pz * 18 + px;
    let col = locate(f.job, x, z);
    let (base, height) = (si as i32 * 16, f.height);
    if f.paths.pile {
        // Bottom and top read one by one, the 16 cells of the section in one go (one division
        // per stack).
        let at = |y: i32| match col {
            Some((cc, lx, lz)) if (0..height).contains(&y) => cc.block(lx, y as usize, lz).unwrap_or(OUTSIDE),
            _ => OUTSIDE,
        };
        state_grid[column_base] = at(base - 1);
        state_grid[column_base + 17 * 324] = at(base + 16);
        match col.and_then(|(cc, lx, lz)| Some((cc.sections.get(si)?, lx, lz))) {
            Some((s, lx, lz)) => {
                for (py, v) in (1..17).zip(s.blocks.column_iter(lx, lz, 0..16)) {
                    state_grid[column_base + py * 324] = v;
                }
            }
            None => (1..17).for_each(|py| state_grid[column_base + py * 324] = OUTSIDE),
        }
    } else {
        for py in 0..18i32 {
            let y = base + py - 1;
            state_grid[column_base + py as usize * 324] = if (0..height).contains(&y) {
                col.and_then(|(c, lx, lz)| c.block(lx, y as usize, lz)).unwrap_or(OUTSIDE)
            } else {
                OUTSIDE
            };
        }
    }
    if f.with_props {
        for py in 0..18 {
            let cell = column_base + py * 324;
            cell_props[cell] = f.cull_tables.prop(f.rule, state_grid[cell]);
        }
    }
}

/// Decodes the sections of the column that this meshing reads: every non-empty one for a full
/// mesh; for a partial one (`remeshed`), the remeshed sections and their neighbours above and
/// below (border of the grids), those reduced to their two border rows if `border_rows_only`.
/// Buffers taken from `scratch` (no allocation per section).
fn unfold_sections(
    c: &Column,
    remeshed: Option<&crate::wire::Sections>,
    border_rows_only: bool,
    scratch: &mut Scratch,
) -> Vec<UnfoldedSection> {
    c.sections
        .iter()
        .enumerate()
        .map(|(si, s)| {
            let (full, border) = match remeshed {
                None => (true, false),
                Some(m) => {
                    (m.contains(si), m.contains(si + 1) || si.checked_sub(1).is_some_and(|b| m.contains(b)))
                }
            };
            if s.is_empty() || !(full || border) {
                return UnfoldedSection::Absent;
            }
            if !full && border_rows_only {
                let mut rows = scratch.border_rows.pop().unwrap_or_else(|| Box::new([0u32; 2 * ROW_BLOCKS]));
                let (bottom, top) = rows.split_at_mut(ROW_BLOCKS);
                s.blocks.unpack_range(0, bottom);
                s.blocks.unpack_range(15 * ROW_BLOCKS, top);
                return UnfoldedSection::BorderRows(rows);
            }
            let mut v = scratch.boxes.pop().unwrap_or_else(|| Box::new([0u32; SECTION_BLOCKS]));
            s.blocks.unpack_into(&mut v[..]);
            UnfoldedSection::Full(v)
        })
        .collect()
}

impl Tables {
    pub fn new(d: &VersionData, models: Arc<BlockModels>) -> Self {
        let air: Vec<bool> = d.states.iter().map(|s| s.has(flags::AIR)).collect();
        let covers: Vec<u8> = d
            .states
            .iter()
            .map(|s| {
                if !s.has(flags::CAN_OCCLUDE) {
                    return 0;
                }
                let mut m = 0u8;
                for dir in Dir::ALL {
                    // A box of the occlusion shape covers the whole side: contact and extent 0..1.
                    let (axis, at_max) = match dir {
                        Dir::Down => (1, false),
                        Dir::Up => (1, true),
                        Dir::North => (2, false),
                        Dir::South => (2, true),
                        Dir::West => (0, false),
                        Dir::East => (0, true),
                    };
                    let full = d.shape(s.occlusion).iter().any(|b| {
                        let touches = if at_max { b.max[axis] >= 1.0 } else { b.min[axis] <= 0.0 };
                        touches && (0..3).filter(|&a| a != axis).all(|a| b.min[a] <= 0.0 && b.max[a] >= 1.0)
                    });
                    m |= u8::from(full) << dir as u8;
                }
                m
            })
            .collect();
        let cull_dirs: Vec<u8> = (0..d.states.len())
            .map(|s| {
                let Some(m) = models.states.get(s) else { return ALWAYS };
                let mut bits = 0u8;
                for p in &m.parts {
                    for &(_, start, count) in &p.options {
                        for q in &models.quads[start as usize..(start + count) as usize] {
                            bits |= q.cull.map_or(ALWAYS, |c| 1 << c as u8);
                        }
                    }
                }
                bits
            })
            .collect();
        let fluid = crate::fluid::FluidTables::new(d);
        let leaves: Vec<bool> = d.states.iter().map(|s| d.blocks[s.block as usize].name.ends_with("leaves")).collect();
        let sealed: Vec<bool> = (0..d.states.len())
            .map(|s| {
                !air[s]
                    && covers[s] == 0x3f
                    && cull_dirs[s] & ALWAYS == 0
                    && !leaves[s]
                    && fluid.flow.fluid(s as u32) == crate::fluid::Fluid::None
            })
            .collect();
        let offset: Vec<(u8, f32, f32)> = d
            .states
            .iter()
            .map(|s| {
                let b = &d.blocks[s.block as usize];
                (b.offset_kind, b.offset_h, b.offset_v)
            })
            .collect();
        let (offset_class, offset_classes) = offset_classes(&offset);
        let is_fluid: Vec<bool> =
            (0..d.states.len()).map(|s| fluid.flow.fluid(s as u32) != crate::fluid::Fluid::None).collect();
        let light_info = crate::light::state_info(d);
        let cull = crate::cull::CullTables::new(d, &covers, &cull_dirs, &air, &leaves, &sealed, &is_fluid, &light_info);
        Tables {
            cull,
            sealed,
            cull_dirs,
            solid: d.states.iter().map(|s| s.has(flags::SOLID_RENDER)).collect(),
            offset,
            offset_class,
            offset_classes,
            bucket: face_buckets(&models),
            light_info,
            // Only if the path is used (switch off: neither the memory nor the walk).
            light_faces: light_faces_in_background(&models, crate::light::LightPaths::from_env().facetab),
            leaves,
            models,
            air,
            covers,
            seed_offset: d.states.iter().map(|s| s.seed_offset()).collect(),
            block: d.states.iter().map(|s| s.block).collect(),
            client: d.client_states.clone(),
            fluid,
        }
    }

    /// Face geometry for the lighting, once built (`LightPaths::facetab`); until then the
    /// mesher computes each face's geometry, same output.
    pub fn light_faces(&self) -> Option<&crate::light::FaceTables> {
        self.light_faces.get()
    }

    /// Builds the face geometry for the lighting now whatever the switch, or waits for the
    /// background build (tests and benches, which must measure the path they ask for).
    pub fn build_light_faces(&self) -> &crate::light::FaceTables {
        self.light_faces.get_or_init(|| face_tables_of(&self.models))
    }
}

fn face_tables_of(models: &BlockModels) -> crate::light::FaceTables {
    crate::light::FaceTables::new(models.quads.iter().map(|q| (&q.pos, q.dir)))
}

/// Face geometry for the lighting built on a thread of its own when `wanted`: a few milliseconds
/// taken off the resource loading, the meshes before it is ready computing each face's geometry
/// instead (same output). No thread available: the per-face computation stays.
fn light_faces_in_background(
    models: &Arc<BlockModels>,
    wanted: bool,
) -> Arc<std::sync::OnceLock<crate::light::FaceTables>> {
    let cell = Arc::new(std::sync::OnceLock::new());
    if wanted {
        let (filled, models) = (cell.clone(), models.clone());
        let build = move || {
            filled.get_or_init(|| face_tables_of(&models));
        };
        // Not started: the per-face computation, same output.
        let _ = std::thread::Builder::new().name("mesh face geometry".into()).spawn(build);
    }
    cell
}

/// Biome colours by network biome identifier: grass, foliage, dry foliage, water.
pub type BiomePalette = Vec<[u32; 4]>;

/// Biome palette in the order received from the server (`names`), from the biomes of the base
/// pack and the colour palettes. Unknown biome: plains colours.
pub fn biome_palette(d: &VersionData, maps: &Colormaps, names: &[String]) -> BiomePalette {
    names
        .iter()
        .map(|n| {
            let b = d.biomes.iter().find(|b| &b.name == n).cloned().unwrap_or_else(|| rc_data::Biome {
                temperature: 0.8,
                downfall: 0.4,
                water_color: 0x3f76e4,
                ..Default::default()
            });
            let mut grass =
                b.grass_color.unwrap_or_else(|| colormap(maps.grass.as_ref(), b.temperature, b.downfall, 0xff00ff));
            grass = match b.grass_modifier {
                // Dark forest: average with a fixed tint.
                1 => ((grass & 0xfe_fefe) + 0x28_340a) >> 1,
                // Swamp: two colours depending on a position noise (resolved at meshing).
                2 => SWAMP_GRASS,
                _ => grass,
            };
            let foliage =
                b.foliage_color.unwrap_or_else(|| colormap(maps.foliage.as_ref(), b.temperature, b.downfall, 0x48b518));
            let dry = b
                .dry_foliage_color
                .unwrap_or_else(|| colormap(maps.dry_foliage.as_ref(), b.temperature, b.downfall, 0xa0_7e5a));
            [grass, foliage, dry, b.water_color]
        })
        .collect()
}

pub struct BlockMesh {
    pub x: i32,
    pub z: i32,
    pub version: u64,
    /// Faces per layer (opaque, cutout, translucent): `[position, baked face, RGB tint,
    /// light of corners 0-1, light of corners 2-3, occlusion gray of the 4 corners]`.
    pub layers: [Vec<Face>; 3],
    /// Fluid faces per layer (explicit geometry, variable heights).
    pub fluids: [Vec<crate::fluid::FluidQuad>; 3],
    /// Start of the faces per layer, group (`BUCKETS`, visible direction) and section:
    /// `g × (S + 1) + s`, with a closing entry per group.
    pub starts: [Vec<u32>; 3],
    pub fluid_starts: [Vec<u32>; 3],
    /// Communication between the faces of each section (`visibility::section_vis`).
    pub vis: Vec<crate::visibility::Vis>,
}

impl BlockMesh {
    pub fn len(&self) -> usize {
        self.layers.iter().map(Vec::len).sum::<usize>() + self.fluids.iter().map(Vec::len).sum::<usize>()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Fingerprint of a mesh (64-bit FNV-1a over all its fields, in order): check of the
/// partial remeshing in game (`RUSTCRAFT_MESHCHECK`) and equivalence proof (`tests/digest.rs`).
pub fn digest(m: &BlockMesh) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut eat = |v: u64| h = (h ^ v).wrapping_mul(0x100_0000_01b3);
    for l in 0..3 {
        eat(m.layers[l].len() as u64);
        m.layers[l].iter().flatten().for_each(|&v| eat(u64::from(v)));
        m.starts[l].iter().chain(&m.fluid_starts[l]).for_each(|&v| eat(u64::from(v)));
        eat(m.fluids[l].len() as u64);
        for q in &m.fluids[l] {
            q.corners.iter().flatten().for_each(|c| eat(u64::from(c.to_bits())));
            eat(u64::from(q.texture));
            q.colors.iter().for_each(|&c| eat(u64::from(c)));
            q.light.iter().for_each(|&c| eat(u64::from(c)));
        }
    }
    m.vis.iter().for_each(|&v| eat(v));
    h
}

const fn opposite(d: Dir) -> Dir {
    match d {
        Dir::Down => Dir::Up,
        Dir::Up => Dir::Down,
        Dir::North => Dir::South,
        Dir::South => Dir::North,
        Dir::West => Dir::East,
        Dir::East => Dir::West,
    }
}

/// Rank in `NEIGHBORS` of the column with offset (dx, dz), cell (dz + 1) × 3 + dx + 1; 8 = centre.
const NEIGHBOR_SLOT: [u8; 9] = {
    let mut a = [8u8; 9];
    let mut i = 0;
    while i < 8 {
        let (dx, dz) = crate::wire::NEIGHBORS[i];
        a[((dz + 1) * 3 + dx + 1) as usize] = i as u8;
        i += 1;
    }
    a
};

/// Column and local coordinates for a position relative to the central column
/// (−16 ≤ x, z < 32), or `None` outside the available columns.
pub(crate) fn locate(job: &MeshJob, x: i32, z: i32) -> Option<(&Column, usize, usize)> {
    let (cx, cz) = (x.div_euclid(16), z.div_euclid(16));
    if job.tint.idx {
        // Direct access (`RUSTCRAFT_OPT_TINTIDX`): cell [dz + 1][dx + 1] instead of a search
        // in the neighbour list.
        if !(-1..=1).contains(&cx) || !(-1..=1).contains(&cz) {
            return None;
        }
        let col = match NEIGHBOR_SLOT[((cz + 1) * 3 + cx + 1) as usize] {
            8 => Some(&*job.center),
            i => job.neighbors[i as usize].as_deref(),
        }?;
        return Some((col, x.rem_euclid(16) as usize, z.rem_euclid(16) as usize));
    }
    let col = match (cx, cz) {
        (0, 0) => Some(&*job.center),
        (dx, dz) => {
            let i = crate::wire::NEIGHBORS.iter().position(|&n| n == (dx, dz))?;
            job.neighbors[i].as_deref()
        }
    }?;
    Some((col, x.rem_euclid(16) as usize, z.rem_euclid(16) as usize))
}

/// Palette marker (bit outside the 24 colour bits): swamp grass, whose colour
/// depends on the position (two-tint noise) and not on the biome alone.
pub const SWAMP_GRASS: u32 = 0x0100_0000;
const SWAMP_GRASS_COLD: u32 = 0x4c_763c;
const SWAMP_GRASS_WARM: u32 = 0x6a_7039;

/// Blurred biome zoom: quart (x, y, z in absolute quart coordinates) whose biome
/// applies to the absolute block `(x, y, z)`, for the hashed seed received from the server. Each of the
/// 8 corners of the neighbouring cell is moved by a pseudo-random offset; the nearest wins.
pub fn biome_zoom(seed: i64, x: i32, y: i32, z: i32) -> (i32, i32, i32) {
    let (ax, ay, az) = (x.wrapping_sub(2), y.wrapping_sub(2), z.wrapping_sub(2));
    let (px, py, pz) = (ax >> 2, ay >> 2, az >> 2);
    let corner = zoom_corner(seed, ax, ay, az);
    (px + i32::from(corner & 4 != 0), py + i32::from(corner & 2 != 0), pz + i32::from(corner & 1 != 0))
}

/// Index (bits x = 4, y = 2, z = 1) of the corner retained by the zoom, positions already offset by 2.
fn zoom_corner(seed: i64, ax: i32, ay: i32, az: i32) -> u8 {
    zoom_corner_with(ax, ay, az, |cx, cy, cz| corner_offsets(seed, cx, cy, cz))
}

/// Zoom corner, offsets of the 8 corners read by `offsets` (direct computation, or memo per lattice
/// point): same f64 operations in the same order, hence the same corner.
#[inline]
fn zoom_corner_with(ax: i32, ay: i32, az: i32, mut offsets: impl FnMut(i32, i32, i32) -> [f64; 3]) -> u8 {
    let (px, py, pz) = (ax >> 2, ay >> 2, az >> 2);
    let fx = f64::from(ax & 3) / 4.0;
    let fy = f64::from(ay & 3) / 4.0;
    let fz = f64::from(az & 3) / 4.0;
    let mut best = (0u8, f64::INFINITY);
    for i in 0..8u8 {
        let (xe, ye, ze) = (i & 4 == 0, i & 2 == 0, i & 1 == 0);
        let (cx, cy, cz) = (px + i32::from(!xe), py + i32::from(!ye), pz + i32::from(!ze));
        let dx = if xe { fx } else { fx - 1.0 };
        let dy = if ye { fy } else { fy - 1.0 };
        let dz = if ze { fz } else { fz - 1.0 };
        let [ox, oy, oz] = offsets(cx, cy, cz);
        let d = (dz + oz) * (dz + oz) + (dy + oy) * (dy + oy) + (dx + ox) * (dx + ox);
        if best.1 > d {
            best = (i, d);
        }
    }
    best.0
}

/// Pseudo-random offset (x, y, z) of the lattice point (cx, cy, cz): six LCG steps on the
/// coordinates, then one step per axis; depends only on the seed and the point.
#[inline]
fn corner_offsets(seed: i64, cx: i32, cy: i32, cz: i32) -> [f64; 3] {
    let lcg = |r: i64, c: i64| {
        r.wrapping_mul(r.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407))
            .wrapping_add(c)
    };
    let fiddle = |r: i64| ((r >> 24).rem_euclid(1024) as f64 / 1024.0 - 0.5) * 0.9;
    let mut r = seed;
    for c in [cx, cy, cz, cx, cy, cz] {
        r = lcg(r, i64::from(c));
    }
    let ox = fiddle(r);
    r = lcg(r, seed);
    let oy = fiddle(r);
    r = lcg(r, seed);
    [ox, oy, fiddle(r)]
}

/// Zoom cached per thread (direct-access table): the result only depends on the seed and
/// the absolute position, so an entry is never stale; the blend re-reads each block
/// up to (2r + 1)² times, and the client's main thread re-reads the same blocks every frame
/// (moving blocks, rain columns, biome probe). 8 192 entries of 24 bytes (≈ 196 KB) per thread.
pub fn biome_zoom_cached(seed: i64, x: i32, y: i32, z: i32) -> (i32, i32, i32) {
    const N: usize = 1 << 13;
    thread_local! {
        static CACHE: std::cell::RefCell<Vec<(i64, [i32; 3], u8)>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    let (ax, ay, az) = (x.wrapping_sub(2), y.wrapping_sub(2), z.wrapping_sub(2));
    let h = (x as u32).wrapping_mul(0x9e37_79b1)
        ^ (z as u32).wrapping_mul(0x85eb_ca77)
        ^ (y as u32).wrapping_mul(0xc2b2_ae3d);
    let slot = (h >> 19) as usize & (N - 1);
    let corner = CACHE.with_borrow_mut(|c| {
        if c.is_empty() {
            c.resize(N, (0, [i32::MIN; 3], 0));
        }
        let e = &mut c[slot];
        if e.0 == seed && e.1 == [x, y, z] {
            return e.2;
        }
        let k = zoom_corner(seed, ax, ay, az);
        *e = (seed, [x, y, z], k);
        k
    });
    (
        (ax >> 2) + i32::from(corner & 4 != 0),
        (ay >> 2) + i32::from(corner & 2 != 0),
        (az >> 2) + i32::from(corner & 1 != 0),
    )
}

/// Biome (network identifier) of an absolute quart; ordinate bounded to the world height like
/// the game, `None` if the column is not supplied.
fn biome_at_quart(job: &MeshJob, (qx, qy, qz): (i32, i32, i32)) -> Option<u32> {
    let (col, lx, lz) = locate(job, qx * 4 - job.center.x * 16, qz * 4 - job.center.z * 16)?;
    let top = col.sections.len() as i32 * 4 - 1;
    let ly = (qy - (job.min_y >> 2)).clamp(0, top.max(0));
    let s = col.sections.get((ly >> 2) as usize)?;
    Some(s.biomes.get((((ly & 3) as usize) << 4) | ((lz >> 2) << 2) | (lx >> 2)))
}

/// Swamp grass colour at the absolute block (x, z): biome information noise.
pub fn swamp_grass(x: i32, z: i32) -> u32 {
    if biome_info_noise(f64::from(x) * 0.0225, f64::from(z) * 0.0225) < -0.1 {
        SWAMP_GRASS_COLD
    } else {
        SWAMP_GRASS_WARM
    }
}

/// 2D simplex biome information noise (Java generator with seed 2345: three doubles
/// drawn then the permutation), value ×70 rounded to single precision like the game.
fn biome_info_noise(u: f64, v: f64) -> f64 {
    static PERM: std::sync::OnceLock<[u8; 256]> = std::sync::OnceLock::new();
    let p = PERM.get_or_init(|| {
        let mut r = rc_assets::bake::Lcg::new(2345);
        for _ in 0..3 {
            r.next_double();
        }
        let mut p: [u8; 256] = std::array::from_fn(|i| i as u8);
        for i in 0..256 {
            let j = i + r.next_int(256 - i as u32) as usize;
            p.swap(i, j);
        }
        p
    });
    const GRAD: [(f64, f64); 12] = [
        (1.0, 1.0),
        (-1.0, 1.0),
        (1.0, -1.0),
        (-1.0, -1.0),
        (1.0, 0.0),
        (-1.0, 0.0),
        (1.0, 0.0),
        (-1.0, 0.0),
        (0.0, 1.0),
        (0.0, -1.0),
        (0.0, 1.0),
        (0.0, -1.0),
    ];
    let perm = |i: i32| i32::from(p[(i & 255) as usize]);
    let f2 = 0.5 * (3f64.sqrt() - 1.0);
    let g2 = (3.0 - 3f64.sqrt()) / 6.0;
    let s = (u + v) * f2;
    let i = (u + s).floor() as i32;
    let j = (v + s).floor() as i32;
    let t = f64::from(i.wrapping_add(j)) * g2;
    let x0 = u - (f64::from(i) - t);
    let y0 = v - (f64::from(j) - t);
    let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };
    let x1 = x0 - f64::from(i1) + g2;
    let y1 = y0 - f64::from(j1) + g2;
    let x2 = x0 - 1.0 + 2.0 * g2;
    let y2 = y0 - 1.0 + 2.0 * g2;
    let (ii, jj) = (i & 255, j & 255);
    let g0 = perm(ii + perm(jj)) % 12;
    let g1 = perm(ii + i1 + perm(jj + j1)) % 12;
    let gi2 = perm(ii + 1 + perm(jj + 1)) % 12;
    let corner = |g: i32, x: f64, y: f64| {
        let t = 0.5 - x * x - y * y;
        if t < 0.0 {
            0.0
        } else {
            let t = t * t;
            let (gx, gy) = GRAD[g as usize];
            t * t * (gx * x + gy * y)
        }
    };
    f64::from((70.0 * (corner(g0, x0, y0) + corner(g1, x1, y1) + corner(gi2, x2, y2))) as f32)
}

/// Colour of a block relative to the column (zoomed biome), without blending.
fn tint_at(job: &MeshJob, palette: &BiomePalette, kind: usize, (x, y, z): (i32, i32, i32), cached: bool) -> u32 {
    let (wx, wy, wz) = (job.center.x * 16 + x, job.min_y + y, job.center.z * 16 + z);
    let q = if cached {
        biome_zoom_cached(job.biome_zoom_seed, wx, wy, wz)
    } else {
        biome_zoom(job.biome_zoom_seed, wx, wy, wz)
    };
    // Neighbouring column absent: the game reads an empty level, hence plains.
    let b = biome_at_quart(job, q).unwrap_or(job.plains_biome);
    let c = palette.get(b as usize).map_or(0xff_ffff, |c| c[kind]);
    if c == SWAMP_GRASS { swamp_grass(wx, wz) } else { c }
}

/// Blended biome colour, like the game: integer average of the components over (2r + 1)²
/// blocks at the same height, each block taking the biome of the blurred zoom.
pub(crate) fn blended(job: &MeshJob, palette: &BiomePalette, kind: usize, x: i32, y: i32, z: i32) -> u32 {
    blend_with(job, palette, kind, (x, y, z), true)
}

fn blend_with(job: &MeshJob, palette: &BiomePalette, kind: usize, (x, y, z): (i32, i32, i32), cached: bool) -> u32 {
    let rad = job.opts.blend.clamp(0, 7);
    if rad == 0 {
        return tint_at(job, palette, kind, (x, y, z), cached);
    }
    let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
    for bz in z - rad..=z + rad {
        for bx in x - rad..=x + rad {
            let c = tint_at(job, palette, kind, (bx, y, bz), cached);
            r += (c >> 16) & 0xff;
            g += (c >> 8) & 0xff;
            b += c & 0xff;
        }
    }
    let n = ((2 * rad + 1) * (2 * rad + 1)) as u32;
    ((r / n) << 16) | ((g / n) << 8) | (b / n)
}

/// Default threshold of the sliding sums: requests for one layer (kind, y) before computing it
/// in full.
const BOX_MIN: u16 = 32;

/// Paths of the mesh's biome tint computation. Each returns the colours of [`blend_with`]
/// bit for bit and is turned off by its switch (`=0`: old path), read once at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TintPaths {
    /// Single-biome zone: palette colour without zoom or average (`RUSTCRAFT_OPT_TINTUNI`).
    pub uni: bool,
    /// Zoom offsets memorised per lattice point (`RUSTCRAFT_OPT_TINTLATTICE`).
    pub lattice: bool,
    /// Lazy grid of the zoomed biomes per layer of the section, swamp grass kept per
    /// (x, z) (`RUSTCRAFT_OPT_TINTGRID`).
    pub grid: bool,
    /// Neighbouring column by direct access, biomes per quart kept for the section (`RUSTCRAFT_OPT_TINTIDX`).
    pub idx: bool,
    /// Sliding sums of a whole layer from its `boxsum`-th request; 0 = off
    /// (`RUSTCRAFT_OPT_TINTBOX`, a numeric value fixes the threshold).
    pub boxsum: u16,
}

impl TintPaths {
    pub const ALL: TintPaths = TintPaths { uni: true, lattice: true, grid: true, idx: true, boxsum: BOX_MIN };
    pub const OLD: TintPaths = TintPaths { uni: false, lattice: false, grid: false, idx: false, boxsum: 0 };

    /// Paths chosen by the environment (all active by default).
    pub fn from_env() -> Self {
        static P: std::sync::OnceLock<TintPaths> = std::sync::OnceLock::new();
        *P.get_or_init(|| {
            let on = |v: Option<&std::ffi::OsStr>| v.is_none_or(|v| v != "0");
            TintPaths {
                uni: on(rc_data::env_os!("RUSTCRAFT_OPT_TINTUNI")),
                lattice: on(rc_data::env_os!("RUSTCRAFT_OPT_TINTLATTICE")),
                grid: on(rc_data::env_os!("RUSTCRAFT_OPT_TINTGRID")),
                idx: on(rc_data::env_os!("RUSTCRAFT_OPT_TINTIDX")),
                boxsum: rc_data::env_str!("RUSTCRAFT_OPT_TINTBOX").map_or(BOX_MIN, |v| v.parse().unwrap_or(BOX_MIN)),
            }
        })
    }
}

/// Bounds of the memos per lattice point (quarts) of a section: x and z ≤ 10 (32-block window
/// at radius 7), y ≤ 7 (18 blocks).
const LAT_X: i32 = 10;
const LAT_Y: i32 = 7;
const LAT_N: usize = (LAT_X * LAT_Y * LAT_X) as usize;
/// Maximum side of the window of blocks read by the blend (18 + 2 × 7), and y layers of a section.
const GW: usize = 32;
const GL: usize = 18;
const UNSET: u32 = u32::MAX;

/// Tint buffers, kept by the thread (`Scratch`), reset per section.
#[derive(Default)]
struct TintBufs {
    /// y relative to the bottom of the current section.
    base: i32,
    /// Single biome of the reachable zone of the section: `None` not searched yet.
    uni: Option<Option<u32>>,
    /// Origin and size (quarts) of the memos per lattice point; size 0 if out of bounds.
    org: [i32; 3],
    dim: [i32; 3],
    lat: Vec<[f64; 3]>,
    lat_used: bool,
    quart: Vec<u32>,
    quart_used: bool,
    /// Zoomed biome per (layer, z, x) of the window; layers filled (bits).
    grid: Vec<u32>,
    grid_dirty: u32,
    /// Swamp grass per (z, x) of the window (does not depend on y: kept for the column).
    swamp: Vec<u32>,
    swamp_used: bool,
    /// Blended colours per (kind × 18 + layer, z, x); layers computed (bits) and requests.
    boxv: Vec<u32>,
    box_done: u128,
    box_count: Vec<u16>,
}

// Test sabotages: 1 grid window shifted by one block, 2 y outside the section not folded,
// 3 sliding-sum window shifted by one block, 4 single-biome zone without excluding the
// swamp, 5 lattice offset memo not reset between sections.
#[cfg(test)]
thread_local! {
    static SABOTAGE: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}
#[cfg(test)]
fn sabotage() -> u8 {
    SABOTAGE.get()
}
#[cfg(not(test))]
#[inline(always)]
fn sabotage() -> u8 {
    0
}

/// Biome tints of a mesh (same colours as [`blended`]): single-biome zone,
/// per-section memos of the zoom offsets, of the biomes per quart and of the zoomed biomes, sliding
/// sums of the dense layers; explicit fallback to direct computation outside the section.
pub struct Tint<'a> {
    job: &'a MeshJob,
    palette: &'a BiomePalette,
    paths: TintPaths,
    rad: i32,
    /// False: old computation (`blended`) for any request (reference version).
    on: bool,
    bufs: std::cell::RefCell<TintBufs>,
}

/// Components of a colour on three 21-bit lanes (sum of 225 bytes without carry).
#[inline]
fn lanes(c: u32) -> u64 {
    u64::from((c >> 16) & 0xff) << 42 | u64::from((c >> 8) & 0xff) << 21 | u64::from(c & 0xff)
}

impl<'a> Tint<'a> {
    fn new(job: &'a MeshJob, palette: &'a BiomePalette, paths: TintPaths, on: bool, mut bufs: TintBufs) -> Self {
        if bufs.swamp_used {
            bufs.swamp.fill(UNSET);
            bufs.swamp_used = false;
        }
        Tint { job, palette, paths, rad: job.opts.blend.clamp(0, 7), on, bufs: bufs.into() }
    }

    fn into_bufs(self) -> TintBufs {
        self.bufs.into_inner()
    }

    /// Start of the section whose bottom is at `base` (relative y): memos reset.
    fn section(&self, base: i32) {
        if !self.on {
            return;
        }
        let s = &mut *self.bufs.borrow_mut();
        let (p, j, r) = (self.paths, self.job, self.rad);
        s.base = base;
        s.uni = None;
        let lo = |a: i32| a.wrapping_sub(2) >> 2;
        let (wx, wz, wy) = (j.center.x * 16, j.center.z * 16, j.min_y + base);
        s.org = [lo(wx - 1 - r), lo(wy - 1), lo(wz - 1 - r)];
        s.dim = [lo(wx + 16 + r) + 2 - s.org[0], lo(wy + 16) + 2 - s.org[1], lo(wz + 16 + r) + 2 - s.org[2]];
        if s.dim[0] > LAT_X || s.dim[1] > LAT_Y || s.dim[2] > LAT_X {
            s.dim = [0; 3];
        }
        if p.lattice && s.lat.len() != LAT_N {
            s.lat = vec![[f64::NAN; 3]; LAT_N];
        } else if s.lat_used && sabotage() != 5 {
            s.lat.fill([f64::NAN; 3]);
        }
        s.lat_used = false;
        if p.idx && s.quart.len() != LAT_N {
            s.quart = vec![UNSET; LAT_N];
        } else if s.quart_used {
            s.quart.fill(UNSET);
        }
        s.quart_used = false;
        if p.grid {
            if s.grid.len() != GL * GW * GW {
                s.grid = vec![UNSET; GL * GW * GW];
                s.swamp = vec![UNSET; GW * GW];
            }
            while s.grid_dirty != 0 {
                let l = s.grid_dirty.trailing_zeros() as usize;
                s.grid[l * GW * GW..(l + 1) * GW * GW].fill(UNSET);
                s.grid_dirty &= s.grid_dirty - 1;
            }
        }
        if p.boxsum > 0 {
            if s.boxv.len() != 4 * GL * 324 {
                s.boxv = vec![0; 4 * GL * 324];
                s.box_count = vec![0; 4 * GL];
            }
            s.box_done = 0;
            s.box_count.fill(0);
        }
    }

    /// Memo cell per lattice point (absolute quart `q`), `None` outside the section's zone.
    #[inline]
    fn lat_index(s: &TintBufs, q: [i32; 3]) -> Option<usize> {
        let d = [q[0] - s.org[0], q[1] - s.org[1], q[2] - s.org[2]];
        ((0..s.dim[0]).contains(&d[0]) && (0..s.dim[1]).contains(&d[1]) && (0..s.dim[2]).contains(&d[2]))
            .then(|| ((d[1] * LAT_X + d[2]) * LAT_X + d[0]) as usize)
    }

    /// Biome (plains for an absent column) of absolute quart `q`.
    #[inline]
    fn quart_biome(&self, s: &mut TintBufs, q: [i32; 3]) -> u32 {
        let direct = || biome_at_quart(self.job, (q[0], q[1], q[2])).unwrap_or(self.job.plains_biome);
        if !self.paths.idx {
            return direct();
        }
        let Some(i) = Self::lat_index(s, q) else { return direct() };
        if s.quart[i] == UNSET {
            s.quart[i] = direct();
            s.quart_used = true;
        }
        s.quart[i]
    }

    /// Zoomed biome of the absolute block (wx, wy, wz).
    fn zoomed(&self, s: &mut TintBufs, (wx, wy, wz): (i32, i32, i32)) -> u32 {
        let seed = self.job.biome_zoom_seed;
        let q = if self.paths.lattice {
            let (ax, ay, az) = (wx.wrapping_sub(2), wy.wrapping_sub(2), wz.wrapping_sub(2));
            let k = zoom_corner_with(ax, ay, az, |cx, cy, cz| match Self::lat_index(s, [cx, cy, cz]) {
                Some(i) => {
                    if s.lat[i][0].is_nan() {
                        s.lat[i] = corner_offsets(seed, cx, cy, cz);
                        s.lat_used = true;
                    }
                    s.lat[i]
                }
                None => corner_offsets(seed, cx, cy, cz),
            });
            [(ax >> 2) + i32::from(k & 4 != 0), (ay >> 2) + i32::from(k & 2 != 0), (az >> 2) + i32::from(k & 1 != 0)]
        } else {
            let (x, y, z) = biome_zoom_cached(seed, wx, wy, wz);
            [x, y, z]
        };
        self.quart_biome(s, q)
    }

    /// Zoomed biome of the block (relative x, layer `ly` of the section, relative z).
    fn cell(&self, s: &mut TintBufs, x: i32, ly: i32, z: i32) -> u32 {
        let j = self.job;
        let r = self.rad;
        let at = |s: &TintBufs, x: i32| (j.center.x * 16 + x, j.min_y + s.base - 1 + ly, j.center.z * 16 + z);
        if self.paths.grid {
            let (gx, gz) = (x + 1 + r, z + 1 + r);
            let gw = 18 + 2 * r;
            if (0..gw).contains(&gx) && (0..gw).contains(&gz) {
                let i = (ly as usize * GW + gz as usize) * GW + gx as usize;
                if s.grid[i] == UNSET {
                    let w = at(s, if sabotage() == 1 { x + 1 } else { x });
                    s.grid[i] = self.zoomed(s, w);
                    s.grid_dirty |= 1 << ly;
                }
                return s.grid[i];
            }
        }
        let w = at(s, x);
        self.zoomed(s, w)
    }

    /// Palette colour of biome `b` for kind `kind` at relative block (x, z).
    #[inline]
    fn color(&self, s: &mut TintBufs, kind: usize, b: u32, x: i32, z: i32) -> u32 {
        let c = self.palette.get(b as usize).map_or(0xff_ffff, |c| c[kind]);
        if c != SWAMP_GRASS {
            return c;
        }
        let (wx, wz) = (self.job.center.x * 16 + x, self.job.center.z * 16 + z);
        let r = self.rad;
        let (gx, gz) = (x + 1 + r, z + 1 + r);
        if self.paths.grid && (0..18 + 2 * r).contains(&gx) && (0..18 + 2 * r).contains(&gz) {
            let i = gz as usize * GW + gx as usize;
            if s.swamp[i] == UNSET {
                s.swamp[i] = swamp_grass(wx, wz);
                s.swamp_used = true;
            }
            return s.swamp[i];
        }
        swamp_grass(wx, wz)
    }

    /// A single biome over all the quarts that the zoom can return for the blocks read by the section.
    fn uniform(&self, s: &mut TintBufs) -> Option<u32> {
        if s.dim[0] == 0 {
            return None;
        }
        let mut first = None;
        for dz in 0..s.dim[2] {
            for dx in 0..s.dim[0] {
                for dy in 0..s.dim[1] {
                    let b = self.quart_biome(s, [s.org[0] + dx, s.org[1] + dy, s.org[2] + dz]);
                    if *first.get_or_insert(b) != b {
                        return None;
                    }
                }
            }
        }
        first
    }

    /// Whole layer (kind, `ly`) by separable sliding sums: same integer sums.
    fn box_layer(&self, s: &mut TintBufs, kind: usize, ly: i32) {
        let r = self.rad;
        let (w, span) = ((18 + 2 * r) as usize, (2 * r + 1) as usize);
        let shift = i32::from(sabotage() == 3);
        let mut rows = [[0u64; 18]; GW];
        for (zz, row) in rows.iter_mut().enumerate().take(w) {
            let z = zz as i32 - 1 - r;
            let mut line = [0u64; GW];
            for (xx, v) in line.iter_mut().enumerate().take(w) {
                let x = xx as i32 - 1 - r + shift;
                let b = self.cell(s, x, ly, z);
                *v = lanes(self.color(s, kind, b, x, z));
            }
            let mut acc: u64 = line[..span].iter().sum();
            row[0] = acc;
            for ox in 1..18 {
                acc = acc + line[ox + span - 1] - line[ox - 1];
                row[ox] = acc;
            }
        }
        let n = (span * span) as u64;
        let out = &mut s.boxv[(kind * GL + ly as usize) * 324..][..324];
        for ox in 0..18 {
            let mut acc: u64 = rows[..span].iter().map(|r| r[ox]).sum();
            for oz in 0..18 {
                if oz > 0 {
                    acc = acc + rows[oz + span - 1][ox] - rows[oz - 1][ox];
                }
                let ch = |sh: u32| ((acc >> sh) & 0x1f_ffff) / n;
                out[oz * 18 + ox] = (ch(42) << 16 | ch(21) << 8 | ch(0)) as u32;
            }
        }
    }

    /// Blended colour of kind `kind` at relative block (x, y, z), like [`blended`].
    pub fn get(&self, kind: usize, x: i32, y: i32, z: i32) -> u32 {
        if !self.on {
            return blended(self.job, self.palette, kind, x, y, z);
        }
        let s = &mut *self.bufs.borrow_mut();
        let mut ly = y - s.base + 1;
        if sabotage() == 2 {
            ly = ly.clamp(0, GL as i32 - 1);
        }
        // Explicit fallback: position outside the section and its one-block border.
        if !(0..GL as i32).contains(&ly) || !(-1..17).contains(&x) || !(-1..17).contains(&z) {
            return blended(self.job, self.palette, kind, x, y, z);
        }
        let r = self.rad;
        if self.paths.uni {
            let u = match s.uni {
                Some(u) => u,
                None => {
                    let u = self.uniform(s);
                    s.uni = Some(u);
                    u
                }
            };
            if let Some(b) = u {
                // Integer average of equal values: the value (without the high bits beyond r = 0).
                let c = self.palette.get(b as usize).map_or(0xff_ffff, |c| c[kind]);
                if c != SWAMP_GRASS || sabotage() == 4 {
                    return if r == 0 { c } else { c & 0xff_ffff };
                }
            }
        }
        if r == 0 {
            let b = self.cell(s, x, ly, z);
            return self.color(s, kind, b, x, z);
        }
        if self.paths.boxsum > 0 {
            let k = kind * GL + ly as usize;
            if s.box_done >> k & 1 == 0 {
                s.box_count[k] += 1;
                if s.box_count[k] >= self.paths.boxsum {
                    self.box_layer(s, kind, ly);
                    s.box_done |= 1 << k;
                }
            }
            if s.box_done >> k & 1 == 1 {
                return s.boxv[k * 324 + ((z + 1) * 18 + x + 1) as usize];
            }
        }
        let (mut sr, mut sg, mut sb) = (0u32, 0u32, 0u32);
        for bz in z - r..=z + r {
            for bx in x - r..=x + r {
                let b = self.cell(s, bx, ly, bz);
                let c = self.color(s, kind, b, bx, bz);
                sr += (c >> 16) & 0xff;
                sg += (c >> 8) & 0xff;
                sb += c & 0xff;
            }
        }
        let n = ((2 * r + 1) * (2 * r + 1)) as u32;
        ((sr / n) << 16) | ((sg / n) << 8) | (sb / n)
    }
}

/// Check: [`Tint`] with each of the `paths` equals the direct computation without cache for each
/// section, all kinds, layers `ystep` by `ystep` of the section and its border starting
/// one block beyond (fallback), x and z from −2 to 17, requests in a shuffled order. The buffers of the
/// first path are taken back from the thread (reset from one call to the next proven). Returns the
/// first difference found.
#[doc(hidden)]
pub fn tint_matches_reference(job: &MeshJob, palette: &BiomePalette, paths: &[TintPaths], ystep: usize) -> Option<String> {
    let mut first = Some(SCRATCH.with_borrow_mut(|s| std::mem::take(&mut s.tint)));
    let tints: Vec<Tint> =
        paths.iter().map(|&p| Tint::new(job, palette, p, true, first.take().unwrap_or_default())).collect();
    let mut bad = None;
    'all: for si in 0..job.center.sections.len() as i32 {
        let base = si * 16;
        tints.iter().for_each(|t| t.section(base));
        let ys: Vec<i32> = (base - 2..=base + 17).step_by(ystep.max(1)).collect();
        let total = 4 * ys.len() * 20 * 20;
        let stride = [7919usize, 7907, 7901].into_iter().find(|&p| !total.is_multiple_of(p)).unwrap_or(1);
        for i in 0..total {
            let k = i * stride % total;
            let (kind, rest) = (k % 4, k / 4);
            let (x, rest) = ((rest % 20) as i32 - 2, rest / 20);
            let (z, yi) = ((rest % 20) as i32 - 2, rest / 20);
            let y = ys[yi];
            let want = blend_with(job, palette, kind, (x, y, z), false);
            for (t, p) in tints.iter().zip(paths) {
                let got = t.get(kind, x, y, z);
                if got != want {
                    bad = Some(format!("{p:?}: kind {kind} ({x}, {y}, {z}) 0x{got:06x} instead of 0x{want:06x}"));
                    break 'all;
                }
            }
        }
    }
    if let Some(t) = tints.into_iter().next() {
        let bufs = t.into_bufs();
        SCRATCH.with_borrow_mut(|s| s.tint = bufs);
    }
    bad
}

/// Random offset of a block's model (plants, bamboo…), like the game: seed of the
/// position (x, 0, z), 4 bits per axis; horizontal bounded by the block, vertical if the kind provides it.
fn model_offset((kind, max_h, max_v): (u8, f32, f32), world: [i32; 3]) -> Option<[f32; 3]> {
    if kind == 0 {
        return None;
    }
    let n = offset_nibbles(world);
    Some(std::array::from_fn(|a| offset_axis((kind, max_h, max_v), a, n[a])))
}

/// Nibbles of the position seed that set the offset (x, y, z) of a model.
fn offset_nibbles(world: [i32; 3]) -> [u32; 3] {
    let seed = rc_assets::bake::position_seed(world[0], 0, world[2]);
    [(seed & 15) as u32, ((seed >> 4) & 15) as u32, ((seed >> 8) & 15) as u32]
}

/// Offset of the model on axis `axis` (x 0, y 1, z 2) for nibble `n` of the seed, like
/// the game: horizontal (n / 15 − 0.5) × 0.5 bounded, vertical (n / 15 − 1) × bound (kind 2).
fn offset_axis((kind, max_h, max_v): (u8, f32, f32), axis: usize, n: u32) -> f32 {
    let f = (n as f32 / 15.0) as f64;
    match axis {
        1 if kind == 2 => ((f - 1.0) * f64::from(max_v)) as f32,
        1 => 0.0,
        _ => ((f - 0.5) * 0.5).clamp(-f64::from(max_h), f64::from(max_h)) as f32,
    }
}

/// Number of offset classes of the compact faces (4 bits, 0 = none).
pub const OFFSET_CLASSES: usize = 15;
/// Bits of the baked face index in word 1 of a compact offset face; above it, the
/// x, y, z nibbles of the offset (bits 20-31). Its class is in bits 28-31 of word 0
/// (position: x 0-3, z 4-7, y 8-19, column in the region 20-27 set at upload).
pub const QUAD_BITS: u32 = 20;
pub const QUAD_MASK: u32 = (1 << QUAD_BITS) - 1;
const CLASS_SHIFT: u32 = 28;

/// Classes of the offset triples (in order of first appearance per state).
fn offset_classes(offset: &[(u8, f32, f32)]) -> (Vec<u8>, Vec<(u8, f32, f32)>) {
    let mut classes: Vec<(u8, f32, f32)> = Vec::new();
    let per_state = offset
        .iter()
        .map(|&o| {
            if o.0 == 0 {
                return 0;
            }
            let same = |c: &(u8, f32, f32)| c.0 == o.0 && c.1.to_bits() == o.1.to_bits() && c.2.to_bits() == o.2.to_bits();
            let i = classes.iter().position(same).unwrap_or_else(|| {
                classes.push(o);
                classes.len() - 1
            });
            if i < OFFSET_CLASSES { i as u8 + 1 } else { 0 }
        })
        .collect();
    classes.truncate(OFFSET_CLASSES);
    (per_state, classes)
}

impl Tables {
    /// Offset table for the shader: `[class − 1][axis][nibble]` (15 × 3 × 16 floats,
    /// zeros for the absent classes), same values as the explicit path.
    pub fn offset_table(&self) -> Vec<f32> {
        let mut t = vec![0.0f32; OFFSET_CLASSES * 3 * 16];
        for (c, &o) in self.offset_classes.iter().enumerate() {
            for a in 0..3 {
                for n in 0..16 {
                    t[(c * 3 + a) * 16 + n] = offset_axis(o, a, n as u32);
                }
            }
        }
        t
    }

    /// Each offset model fits in a compact face (class and baked face index): no explicit
    /// quad comes from an offset outside translucents.
    pub fn offsets_all_compact(&self) -> bool {
        self.models.quads.len() <= QUAD_MASK as usize + 1
            && self.offset.iter().zip(&self.offset_class).all(|(o, &c)| o.0 == 0 || c != 0)
    }
}

/// Baked face index of a face (masked only for a compact offset face).
pub fn quad_of(f: &Face) -> u32 {
    if f[0] >> CLASS_SHIFT != 0 { f[1] & QUAD_MASK } else { f[1] }
}

/// Words 0 and 1 of a compact offset face: class in the high bits of the position; baked
/// face index and nibbles. `None` if the index or the class does not fit (explicit quad).
fn offset_words(pos: u32, q: u32, class: u8, world: [i32; 3]) -> Option<(u32, u32)> {
    if class == 0 || q > QUAD_MASK {
        return None;
    }
    let n = offset_nibbles(world);
    Some((pos | u32::from(class) << CLASS_SHIFT, q | n[0] << 20 | n[1] << 24 | n[2] << 28))
}

/// Explicit quad equivalent to a compact offset face, computed like the shader (`vs` of
/// `terrain.wgsl`): corner = (block + offset) + baked corner, in f32 in that order; colour
/// = gray × shading then × tint (`vertex_color`). `None` for a face without offset.
pub fn offset_face_quad(t: &Tables, table: &[f32], f: &Face, shade: &[f32; 6]) -> Option<crate::fluid::FluidQuad> {
    let class = (f[0] >> CLASS_SHIFT) as usize;
    if class == 0 {
        return None;
    }
    let quad = &t.models.quads[(f[1] & QUAD_MASK) as usize];
    let n = [(f[1] >> 20) & 15, (f[1] >> 24) & 15, (f[1] >> 28) & 15];
    let o: [f32; 3] = std::array::from_fn(|a| table[((class - 1) * 3 + a) * 16 + n[a] as usize]);
    let block = [(f[0] & 15) as f32, ((f[0] >> 8) & 4095) as f32, ((f[0] >> 4) & 15) as f32];
    let s = shade[quad.shade_dir as usize];
    Some(crate::fluid::FluidQuad {
        corners: std::array::from_fn(|i| {
            let p = quad.pos[i];
            [(block[0] + o[0]) + p[0], (block[1] + o[1]) + p[1], (block[2] + o[2]) + p[2], quad.uv[i][0], quad.uv[i][1]]
        }),
        texture: quad.texture,
        colors: std::array::from_fn(|i| vertex_color((f[5] >> (i * 8)) as u8, s, f[2])),
        light: [f[3] as u16, (f[3] >> 16) as u16, f[4] as u16, (f[4] >> 16) as u16],
    })
}

/// Value of the border grid outside the available columns.
pub(crate) const OUTSIDE: u32 = u32::MAX;

/// States read in the border grid of the current section when the position falls there,
/// in the columns otherwise.
struct Padded<'a> {
    pad: &'a [u32],
    base_y: i32,
    around: &'a Around<'a>,
}

impl crate::fluid::Blocks for Padded<'_> {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        let ly = y - self.base_y;
        if (-1..17).contains(&x) && (-1..17).contains(&z) && (-1..17).contains(&ly) {
            let v = self.pad[((ly + 1) * 324 + (z + 1) * 18 + x + 1) as usize];
            return (v != OUTSIDE).then_some(v);
        }
        self.around.state(x, y, z)
    }
}

/// Bench check: `blended` (cached zoom) equals the direct computation over the whole column
/// (and its border).
#[doc(hidden)]
pub fn blend_matches_reference(job: &MeshJob, palette: &BiomePalette) -> bool {
    let h = job.center.sections.len() as i32 * 16;
    (-1..h + 1).step_by(3).all(|y| {
        (-1..17).all(|z| {
            (-1..17).all(|x| {
                (0..4).all(|k| blended(job, palette, k, x, y, z) == blend_with(job, palette, k, (x, y, z), false))
            })
        })
    })
}
/// States around the column: decompressed sections at the centre, neighbours otherwise.
struct Around<'a> {
    job: &'a MeshJob,
    unpacked: &'a [UnfoldedSection],
    height: i32,
}

impl crate::fluid::Blocks for Around<'_> {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        if !(0..self.height).contains(&y) {
            return None;
        }
        if (0..16).contains(&x) && (0..16).contains(&z) {
            return Some(match &self.unpacked[(y >> 4) as usize] {
                UnfoldedSection::Full(v) => v[((y as usize & 15) << 8) | ((z as usize) << 4) | x as usize],
                // Empty section, not decompressed or only its border rows (partial remeshing):
                // read from the column.
                _ => self.job.center.block(x as usize, y as usize, z as usize).unwrap_or(0),
            });
        }
        let (col, lx, lz) = locate(self.job, x, z)?;
        col.block(lx, y as usize, lz)
    }
}

pub fn mesh(job: &MeshJob, t: &Tables, palette: &BiomePalette, min_y: i32) -> BlockMesh {
    mesh_with(job, t, palette, min_y, true)
}

/// Version without shortcuts, reference of the benches (identical output expected).
pub fn mesh_reference(job: &MeshJob, t: &Tables, palette: &BiomePalette, min_y: i32) -> BlockMesh {
    mesh_with(job, t, palette, min_y, false)
}

fn mesh_with(job: &MeshJob, t: &Tables, palette: &BiomePalette, min_y: i32, fast: bool) -> BlockMesh {
    let c = &job.center;
    let height = c.sections.len() as i32 * 16;
    // Partial remeshing: previous mesh of the same column, of the same height.
    let reuse = job
        .reuse
        .as_ref()
        .and_then(|(m, w)| Some((*m, PackedView::new(w)?)))
        .filter(|(_, b)| b.x == c.x && b.z == c.z && b.sections == c.sections.len());
    let cull_paths = job.cull;
    // Unfolded sections and border grids in buffers kept by the thread (no
    // 16 KiB allocation per section at each meshing).
    let mut scratch = SCRATCH.with_borrow_mut(std::mem::take);
    let unpacked = unfold_sections(c, reuse.as_ref().map(|(m, _)| m), cull_paths.border_rows_only, &mut scratch);
    let around = Around { job, unpacked: &unpacked, height };
    let state_at = |x, y, z| around.state(x, y, z);
    let mut fluids: [Vec<crate::fluid::FluidQuad>; 3] = Default::default();
    let fluid_layer = |f| match f {
        crate::fluid::Fluid::Water => t.models.fluids.water_layer as usize,
        _ => t.models.fluids.lava_layer as usize,
    };

    let models = &t.models;
    let light_view = crate::light::LightView::new(job, &t.light_info);
    let paths = job.light;
    // Precomputed face geometry for the lighting, when its path is on and the table is built.
    let face_tables = if paths.facetab { t.light_faces() } else { None };
    let mut lcache = crate::light::AnyCache::new(paths, &mut scratch.light);
    let mut selected = Vec::new();
    let tint = Tint::new(job, palette, job.tint, fast, std::mem::take(&mut scratch.tint));
    // States of the section and its one-block border (18³ grid): "buried block" test, face
    // removal and fluids read their neighbours there without going through the columns.
    let mut state_grid = std::mem::take(&mut scratch.pad);
    state_grid.clear();
    state_grid.resize(P3, OUTSIDE);
    // Per-cell properties of the same grid (`cull::CullPaths::props`), removal rule chosen.
    let use_props = fast && cull_paths.uses_props();
    let rule = usize::from(cull_paths.shape);
    let cull_tables = &t.cull;
    // Fluids: same removal rule as the blocks (the game's face shapes, or old).
    let shapes = cull_paths.shape.then_some(cull_tables);
    let mut cell_props = std::mem::take(&mut scratch.props);
    let mut candidates = std::mem::take(&mut scratch.cand);
    let mut candidate_sure = std::mem::take(&mut scratch.sure);
    if use_props {
        cell_props.clear();
        cell_props.resize(P3, 0);
    }
    let grid_fill = GridFill {
        job,
        sections: &unpacked,
        height,
        paths: cull_paths,
        cull_tables,
        rule,
        with_props: use_props,
    };
    let mut faces = ColumnFaces::new(job.faces_by_group, unpacked.len(), std::mem::take(&mut scratch.face_groups));
    let mut fluid_starts: [Vec<u32>; 3] = Default::default();
    let mut vis = Vec::with_capacity(unpacked.len());
    // Partial remeshing: final size close to that of the base, reserved in one go.
    if let Some((_, base)) = &reuse {
        for (l, layer) in fluids.iter_mut().enumerate() {
            faces.reserve(l, base.faces[l].len() / 6 + 64);
            layer.reserve_exact(base.fluids[l].len() / FLUID_WORDS + 16);
        }
    }
    for (si, section) in unpacked.iter().enumerate() {
        faces.begin_section(si);
        for (starts, layer) in fluid_starts.iter_mut().zip(&fluids) {
            starts.push(layer.len() as u32);
        }
        if let Some((mask, base)) = &reuse
            && !mask.contains(si)
        {
            vis.push(copy_reused_section(base, si, &mut faces, &mut fluids));
            continue;
        }
        let UnfoldedSection::Full(values) = section else {
            vis.push(crate::visibility::VIS_ALL);
            continue;
        };
        let solid = fast.then(|| fill_section_grids(&grid_fill, si, values, &mut state_grid, &mut cell_props));
        let solid = solid.flatten().unwrap_or_else(|| crate::visibility::solid_bits(&values[..], &t.solid));
        vis.push(crate::visibility::section_vis_bits(&solid));
        // Sealed section (only covering cubes, covering border): no face.
        if fast && if use_props { sealed_props(&cell_props) } else { sealed_section(&values[..], &state_grid, t) } {
            continue;
        }
        let near = Padded { pad: &state_grid, base_y: si as i32 * 16, around: &around };
        let st = |x: i32, y: i32, z: i32| {
            use crate::fluid::Blocks;
            if fast { near.state(x, y, z) } else { around.state(x, y, z) }.unwrap_or(0)
        };
        let lum = |x: i32, y: i32, z: i32| light_view.lum(x, y, z, st(x, y, z));
        tint.section(si as i32 * 16);
        // Rows: only the blocks that can produce something, in order.
        let rows = use_props && cull_paths.rows;
        let sure_kept = rows && cull_paths.simd_candidates;
        if sure_kept {
            row_candidates_with_sure(&cell_props, &mut candidates, &mut candidate_sure);
        } else if rows {
            row_candidates(&cell_props, &mut candidates);
        }
        let count = if rows { candidates.len() } else { SECTION_BLOCKS };
        // The smoothing decides the raw grid; the cells serve it, and the fluids and flat light
        // when they read the section (`LightPaths::fluid`).
        lcache.reset(&light_view, si as i32 * 16, job.opts.ao, count);
        // The light reads the properties and states of the section's grids when they are filled.
        let grids = use_props.then_some(crate::light::SectionGrids { states: &state_grid, props: &cell_props });
        let light_sources = crate::light::Sources { view: &light_view, state_at: &st, grids };
        for k in 0..count {
            let i = if rows { usize::from(candidates[k]) } else { k };
            let state = values[i];
            // Properties of the cell (grid): air, fluid, faces to remove, neighbour coverages.
            let c0 = pad_index(i);
            let p = if use_props { cell_props[c0] } else { 0 };
            if if use_props { p & crate::cull::AIR_BIT != 0 } else { t.air.get(state as usize).copied().unwrap_or(true) } {
                continue;
            }
            let (x, z, y) = ((i & 15) as i32, ((i >> 4) & 15) as i32, (si * 16 + (i >> 8)) as i32);
            // The fluid first (waterlogged block: the water then the block's model).
            let fk = if use_props && p & crate::cull::FLUID_BIT == 0 {
                crate::fluid::Fluid::None
            } else {
                t.fluid.flow.fluid(state)
            };
            if fk != crate::fluid::Fluid::None {
                let tex = &t.models.fluids;
                let out = &mut fluids[fluid_layer(fk)];
                // Light read through the section cache (same rule as the full read).
                let mut cached = |x: i32, y: i32, z: i32| lcache.lum(&light_view, [x, y, z], &st);
                let mut plain = |x: i32, y: i32, z: i32| lum(x, y, z);
                let l: &mut dyn FnMut(i32, i32, i32) -> u16 = if paths.fluid { &mut cached } else { &mut plain };
                if fast {
                    crate::fluid::mesh_fluid(&near, &t.fluid, &t.covers, shapes, tex, job, &tint, (x, y, z), l, out);
                } else {
                    crate::fluid::mesh_fluid(&around, &t.fluid, &t.covers, shapes, tex, job, &tint, (x, y, z), l, out);
                }
            }
            // Exact shortcut: all the possible faces are removed against neighbours that
            // cover them (buried block) → nothing to draw or tint.
            let need = if use_props {
                (p >> crate::cull::NEED) as u8 & 0x3f | if p & crate::cull::ALWAYS_BIT != 0 { ALWAYS } else { 0 }
            } else {
                t.cull_dirs.get(state as usize).copied().unwrap_or(ALWAYS)
            };
            let client = t.client.get(state as usize);
            // Bit d: a face that is removed towards d is hidden (covering neighbour, or identical block
            // for glasses, ices…). Computed once for the block, read by each face.
            let mut hidden = 0u8;
            // Opaque leaves (`cutoutLeaves` off): solid layer, faces against other leaves removed.
            let opaque_leaves = !job.opts.cutout_leaves && t.leaves.get(state as usize).copied().unwrap_or(false);
            let is_leaves = |n: u32| n != OUTSIDE && t.leaves.get(n as usize).copied().unwrap_or(false);
            if fast {
                if need == 0 {
                    continue;
                }
                if use_props {
                    // Neighbours read in the property grid: sure coverages (kept by the
                    // candidate rows, or computed here), leaves, then (game rule) shape
                    // inclusion for the remaining faces.
                    use crate::cull::{LEAF_BIT, NE};
                    let nb = neighbor_props(&cell_props, c0);
                    hidden = if sure_kept { candidate_sure[k] } else { crate::cull::sure_hidden(p, nb) as u8 };
                    if opaque_leaves {
                        for (d, &n) in nb.iter().enumerate() {
                            hidden |= u8::from(n & LEAF_BIT != 0) << d;
                        }
                    }
                    if cull_paths.shape {
                        let mut open = need & !hidden & 0x3f;
                        while open != 0 {
                            let d = open.trailing_zeros() as usize;
                            open &= open - 1;
                            if p >> (NE + d as u32) & 1 == 1 && nb[d] >> (NE + (d ^ 1) as u32) & 1 == 1 {
                                let n = state_grid[c0.wrapping_add_signed(PAD_STEP[d])];
                                hidden |= u8::from(cull_tables.hidden_by_shape(state, n, d)) << d;
                            }
                        }
                    }
                } else {
                    hidden = Dir::ALL.iter().fold(0u8, |m, &d| {
                        let n = state_grid[c0.wrapping_add_signed(PAD_STEP[d as usize])];
                        let leaf = u8::from(opaque_leaves && is_leaves(n));
                        let cover = if cull_paths.shape {
                            u8::from(n != OUTSIDE && cull_tables.hidden_by_shape(state, n, d as usize))
                        } else {
                            t.covers.get(n as usize).copied().unwrap_or(0) >> opposite(d) as u8 & 1
                        };
                        m | (cover | leaf) << d as u8
                    });
                }
                if need & ALWAYS == 0 && need & !hidden == 0 {
                    continue;
                }
                let skip_self = client.map_or(0, |cs| cs.skip_self) & need & !hidden;
                if skip_self != 0 {
                    let me = t.block.get(state as usize);
                    for d in Dir::ALL {
                        let n = state_grid[c0.wrapping_add_signed(PAD_STEP[d as usize])];
                        if skip_self >> d as u8 & 1 == 1 && n != OUTSIDE && t.block.get(n as usize) == me {
                            hidden |= 1 << d as u8;
                        }
                    }
                }
            }
            let world = [c.x * 16 + x, min_y + y, c.z * 16 + z];
            let offset = model_offset(t.offset.get(state as usize).copied().unwrap_or((0, 0.0, 0.0)), world);
            models.select(state, t.seed_offset[state as usize], world, &mut selected);
            // Smoothed light and ambient occlusion (`ao` setting): block without emission whose
            // first model piece accepts it; otherwise flat light.
            let smooth = job.opts.ao
                && t.light_info.get(state as usize).is_some_and(|i| i & crate::light::info::EMISSION == 0)
                && selected.first().is_some_and(|&q| models.quads[q as usize].ao);
            // Tint of each layer, computed at first need.
            let mut tints = [None::<u32>; 4];
            for &q in &selected {
                let quad = &models.quads[q as usize];
                if let Some(cull) = quad.cull {
                    if fast {
                        if hidden >> cull as u8 & 1 == 1 {
                            continue;
                        }
                    } else if let Some(n) = state_at(x + cull.offset()[0], y + cull.offset()[1], z + cull.offset()[2]) {
                        let covered = if cull_paths.shape {
                            cull_tables.hidden_by_shape(state, n, cull as usize)
                        } else {
                            t.covers.get(n as usize).is_some_and(|m| m >> opposite(cull) as u8 & 1 == 1)
                        };
                        let same = client.is_some_and(|cs| {
                            cs.skip_self >> cull as u8 & 1 == 1
                                && t.block.get(n as usize) == t.block.get(state as usize)
                        });
                        if covered || same || (opaque_leaves && is_leaves(n)) {
                            continue;
                        }
                    }
                }
                let tint = match (usize::try_from(quad.tint).ok().filter(|&l| l < 4), client) {
                    (Some(layer), Some(cs)) => *tints[layer].get_or_insert_with(|| {
                        let v = cs.tint_value[layer];
                        match cs.tint_kind[layer] {
                            tint::CONSTANT => v & 0xff_ffff,
                            k @ (tint::GRASS | tint::FOLIAGE | tint::DRY_FOLIAGE | tint::WATER) => {
                                let off = [(v & 3) as i32 - 1, ((v >> 2) & 3) as i32 - 1, ((v >> 4) & 3) as i32 - 1];
                                let kind = usize::from(k - tint::GRASS);
                                tint.get(kind, x + off[0], y + off[1], z + off[2])
                            }
                            _ => NO_TINT,
                        }
                    }),
                    _ => NO_TINT,
                };
                let geom = crate::light::QuadGeom {
                    pos: &quad.pos,
                    dir: quad.dir,
                    cull: quad.cull,
                    emission: quad.emission,
                    tab: face_tables.and_then(|ft| ft.faces.get(q as usize).map(|f| (f, ft))),
                };
                let verts = lcache.quad_light(&light_sources, [x, y, z], state, &geom, smooth);
                let layer = if opaque_leaves { 0 } else { quad.layer as usize };
                let pos = (x as u32) | ((z as u32) << 4) | ((y as u32) << 8);
                // Offset model outside translucents: compact face (offset re-read by the shader).
                let words = match offset {
                    Some(_) if layer != 2 && job.opts.offset_faces => {
                        offset_words(pos, q, t.offset_class.get(state as usize).copied().unwrap_or(0), world)
                    }
                    Some(_) => None,
                    None => Some((pos, q)),
                };
                match words {
                    Some((w0, w1)) if layer != 2 => {
                        let l = verts.map(|(l, _)| u32::from(l));
                        let g = verts.map(|(_, g)| u32::from(g));
                        faces.push(
                            layer,
                            face_group(&t.bucket, q),
                            [w0, w1, tint, l[0] | l[1] << 16, l[2] | l[3] << 16, g[0] | g[1] << 8 | g[2] << 16 | g[3] << 24],
                        );
                    }
                    _ => {
                        // Translucent or offset model without a compact face: explicit geometry (the
                        // translucent is stored with the water to be sorted with it by distance to the
                        // camera, like the game).
                        let shade = job.shade[quad.shade_dir as usize];
                        let (fx, fy, fz) = (x as f32, y as f32, z as f32);
                        let o = offset.unwrap_or([0.0; 3]);
                        let (fx, fy, fz) = (fx + o[0], fy + o[1], fz + o[2]);
                        fluids[layer].push(crate::fluid::FluidQuad {
                            corners: std::array::from_fn(|i| {
                                let p = quad.pos[i];
                                [fx + p[0], fy + p[1], fz + p[2], quad.uv[i][0], quad.uv[i][1]]
                            }),
                            texture: quad.texture,
                            colors: verts.map(|(_, g)| vertex_color(g, shade, tint)),
                            light: verts.map(|(l, _)| l),
                        });
                    }
                }
            }
        }
    }
    for (starts, layer) in fluid_starts.iter_mut().zip(&fluids) {
        starts.push(layer.len() as u32);
    }
    let (layers, starts, face_groups) = faces.finish(&t.bucket);
    scratch.face_groups = face_groups;
    // Buffers handed back to the thread (at most one column of sections kept).
    for section in unpacked {
        match section {
            UnfoldedSection::Full(v) => scratch.boxes.push(v),
            UnfoldedSection::BorderRows(r) => scratch.border_rows.push(r),
            UnfoldedSection::Absent => {}
        }
    }
    scratch.boxes.truncate(32);
    scratch.border_rows.truncate(32);
    scratch.pad = state_grid;
    scratch.props = cell_props;
    scratch.cand = candidates;
    scratch.sure = candidate_sure;
    scratch.tint = tint.into_bufs();
    lcache.finish(&mut scratch.light);
    SCRATCH.with_borrow_mut(|s| *s = scratch);
    BlockMesh { x: c.x, z: c.z, version: job.version, layers, fluids, starts, fluid_starts, vis }
}

/// Section outside the remeshing: its faces taken from the previous mesh `base`, group by group
/// (the stable grouping puts them back in the same place). Returns its visibility.
fn copy_reused_section(
    base: &PackedView<'_>,
    si: usize,
    faces: &mut ColumnFaces,
    fluids: &mut [Vec<crate::fluid::FluidQuad>; 3],
) -> crate::visibility::Vis {
    let sections_plus_one = base.sections + 1;
    for (l, layer_fluids) in fluids.iter_mut().enumerate() {
        let bs = base.starts[l];
        for g in 0..BUCKETS {
            let (a, e) = (bs[g * sections_plus_one + si] as usize, bs[g * sections_plus_one + si + 1] as usize);
            let group = base.faces[l][a * 6..e * 6].as_chunks::<6>().0;
            // A base that also serves as GPU bytes carries the column's tag on every face (`pack_tagged`).
            if group.first().is_some_and(|f| f[0] & GPU_TAG_BITS != 0) {
                faces.extend_group_untagged(l, g, group);
            } else {
                faces.extend_group(l, g, group);
            }
        }
        let fs = base.fluid_starts[l];
        let words = &base.fluids[l][fs[si] as usize * FLUID_WORDS..fs[si + 1] as usize * FLUID_WORDS];
        layer_fluids.extend(words.as_chunks::<FLUID_WORDS>().0.iter().map(decode_fluid));
    }
    u64::from(base.vis[si * 2]) | u64::from(base.vis[si * 2 + 1]) << 32
}

/// Working buffers of the mesher, per thread.
#[derive(Default)]
struct Scratch {
    boxes: Vec<Box<[u32; SECTION_BLOCKS]>>,
    /// Border rows of the neighbours of a partial remesh (`CullPaths::border_rows_only`); boxed
    /// so that they move into `UnfoldedSection` and back without copying their 2 KiB.
    #[allow(clippy::vec_box)]
    border_rows: Vec<Box<[u32; 2 * ROW_BLOCKS]>>,
    pad: Vec<u32>,
    /// Per-cell properties of the grid (`cull::CullPaths::props`), candidate blocks of a section
    /// and their surely hidden faces (`CullPaths::simd_candidates`).
    props: Vec<u32>,
    cand: Vec<u16>,
    sure: Vec<u8>,
    tint: TintBufs,
    /// Light cache cells (`LightPaths::scratch`).
    light: crate::light::LightStores,
    /// Face lists per layer and group (`MeshJob::faces_by_group`), emptied after each mesh.
    face_groups: FaceGroups,
}

/// One face list per layer and group.
type FaceGroups = [[Vec<Face>; BUCKETS]; 3];

/// Capacity a meshing thread keeps in each face list of `FaceGroups` between two meshes (B10):
/// beyond it, the list is shrunk back after the mesh. 8,192 faces = 192 KiB; a column of the
/// reference capture holds at most 3,025 faces in one layer and group (99th percentile 1,862), so
/// the lists stop growing after the first meshes while a rare huge column does not stay held.
const KEPT_FACES_PER_GROUP: usize = 8192;

/// Group of a baked face (`Tables::bucket`), the last group if unknown.
fn face_group(bucket: &[u8], quad: u32) -> usize {
    bucket.get(quad as usize).map_or(BUCKETS - 1, |&b| usize::from(b))
}

/// Faces of the column being meshed, in the order of the final mesh: by layer, then group
/// (`BUCKETS`), then section; within one layer, group and section, in meshing order.
enum ColumnFaces {
    /// One list per layer in section order, sorted by group at the end (`group_faces_by_bucket`):
    /// the old path (`RUSTCRAFT_OPT_MESH_FACES_BY_GROUP=0`).
    SortedAtEnd { layers: [Vec<Face>; 3], section_starts: [Vec<u32>; 3], kept: FaceGroups },
    /// Each face pushed straight into the list of its layer and group (lists kept by the thread),
    /// then each layer assembled once at its exact size: no counting pass, no zeroed buffer, no
    /// scattering copy, no growth of a whole-layer list.
    ByGroup { groups: FaceGroups, starts: [Vec<u32>; 3], sections_plus_one: usize },
}

impl ColumnFaces {
    /// `kept`: the thread's face lists (empty, with their capacity).
    fn new(by_group: bool, sections: usize, kept: FaceGroups) -> Self {
        if by_group {
            let starts = std::array::from_fn(|_| vec![0; BUCKETS * (sections + 1)]);
            ColumnFaces::ByGroup { groups: kept, starts, sections_plus_one: sections + 1 }
        } else {
            ColumnFaces::SortedAtEnd { layers: Default::default(), section_starts: Default::default(), kept }
        }
    }

    /// Room for `n` more faces in `layer` (old path; the kept lists already have room).
    fn reserve(&mut self, layer: usize, n: usize) {
        if let ColumnFaces::SortedAtEnd { layers, .. } = self {
            layers[layer].reserve_exact(n);
        }
    }

    /// Section `s` starts: the faces pushed from now on belong to it.
    fn begin_section(&mut self, s: usize) {
        match self {
            ColumnFaces::SortedAtEnd { layers, section_starts, .. } => {
                for (starts, faces) in section_starts.iter_mut().zip(layers.iter()) {
                    starts.push(faces.len() as u32);
                }
            }
            ColumnFaces::ByGroup { groups, starts, sections_plus_one } => {
                for (starts, groups) in starts.iter_mut().zip(groups.iter()) {
                    for (g, faces) in groups.iter().enumerate() {
                        starts[g * *sections_plus_one + s] = faces.len() as u32;
                    }
                }
            }
        }
    }

    fn push(&mut self, layer: usize, group: usize, face: Face) {
        match self {
            ColumnFaces::SortedAtEnd { layers, .. } => layers[layer].push(face),
            ColumnFaces::ByGroup { groups, .. } => groups[layer][group].push(face),
        }
    }

    /// Faces of one group of a section taken from the previous mesh (all in group `group`).
    fn extend_group(&mut self, layer: usize, group: usize, faces: &[Face]) {
        match self {
            ColumnFaces::SortedAtEnd { layers, .. } => layers[layer].extend_from_slice(faces),
            ColumnFaces::ByGroup { groups, .. } => groups[layer][group].extend_from_slice(faces),
        }
    }

    /// [`ColumnFaces::extend_group`] for faces of a base carrying a GPU tag, taken off.
    fn extend_group_untagged(&mut self, layer: usize, group: usize, faces: &[Face]) {
        let faces = faces.iter().map(untagged);
        match self {
            ColumnFaces::SortedAtEnd { layers, .. } => layers[layer].extend(faces),
            ColumnFaces::ByGroup { groups, .. } => groups[layer][group].extend(faces),
        }
    }

    /// Faces and starts of the mesh (`BlockMesh::layers`, `BlockMesh::starts`), and the thread's
    /// face lists handed back empty.
    fn finish(self, bucket: &[u8]) -> ([Vec<Face>; 3], [Vec<u32>; 3], FaceGroups) {
        match self {
            ColumnFaces::SortedAtEnd { layers, mut section_starts, kept } => {
                let mut starts: [Vec<u32>; 3] = Default::default();
                let mut grouped: [Vec<Face>; 3] = Default::default();
                for l in 0..3 {
                    section_starts[l].push(layers[l].len() as u32);
                    (grouped[l], starts[l]) = group_faces_by_bucket(&layers[l], &section_starts[l], bucket);
                }
                (grouped, starts, kept)
            }
            ColumnFaces::ByGroup { mut groups, mut starts, sections_plus_one } => {
                let layers = std::array::from_fn(|l| assemble_layer(&mut groups[l], &mut starts[l], sections_plus_one));
                (layers, starts, groups)
            }
        }
    }
}

/// One layer of the mesh from its group lists, copied once at its exact size; `starts` (group
/// lengths at each section start) become absolute, with the closing entry of each group. The lists
/// are emptied, and shrunk to `KEPT_FACES_PER_GROUP`.
fn assemble_layer(groups: &mut [Vec<Face>; BUCKETS], starts: &mut [u32], sections_plus_one: usize) -> Vec<Face> {
    let mut layer = Vec::with_capacity(groups.iter().map(Vec::len).sum());
    for (g, faces) in groups.iter_mut().enumerate() {
        let group_starts = &mut starts[g * sections_plus_one..(g + 1) * sections_plus_one];
        group_starts[sections_plus_one - 1] = faces.len() as u32;
        let offset = layer.len() as u32;
        group_starts.iter_mut().for_each(|s| *s += offset);
        layer.extend_from_slice(faces);
        faces.clear();
        faces.shrink_to(KEPT_FACES_PER_GROUP);
    }
    layer
}

thread_local! {
    static SCRATCH: std::cell::RefCell<Scratch> = std::cell::RefCell::new(Scratch::default());
}

/// Sets the generation of the light cache kept by this thread (tests of the wrap to the maximum).
#[doc(hidden)]
pub fn set_light_epoch(e: u32) {
    SCRATCH.with_borrow_mut(|s| s.light.set_epoch(e));
}

/// Section with no visible face: all its blocks are sealed (`Tables::sealed`, so each one
/// hidden by its interior neighbours) and every cell of the border (18³ grid) covers the side
/// it touches. Same result as the block-by-block loop, without it.
fn sealed_section(values: &[u32], pad: &[u32], t: &Tables) -> bool {
    if !values.iter().all(|&s| t.sealed.get(s as usize).copied().unwrap_or(false)) {
        return false;
    }
    let covers =
        |i: usize, d: Dir| pad[i] != OUTSIDE && t.covers.get(pad[i] as usize).is_some_and(|m| m >> d as u8 & 1 == 1);
    (1..17).all(|a| {
        (1..17).all(|b| {
            // Borders below, above, north, south, west, east (step y 324, z 18, x 1): each
            // cell must cover the side facing the section.
            covers(a * 18 + b, Dir::Up)
                && covers(17 * 324 + a * 18 + b, Dir::Down)
                && covers(a * 324 + b, Dir::South)
                && covers(a * 324 + 17 * 18 + b, Dir::North)
                && covers(a * 324 + b * 18, Dir::East)
                && covers(a * 324 + b * 18 + 17, Dir::West)
        })
    })
}

/// Flat mesh, base of the partial remeshes (`MeshJob::reuse`): stored as is by
/// the caller (outside the heaps of the meshing threads) and read without being unfolded.
pub type Packed = Arc<dyn std::ops::Deref<Target = [u32]> + Send + Sync>;

/// Words of a fluid face: corners (20 floats), texture, colours (4), lights (2 × 2).
const FLUID_WORDS: usize = 27;

/// Bits 20-27 of word 0 of a face: never set by the mesher (the position takes bits 0-19, the
/// offset class 28-31). The client writes there the column's place in its GPU region; a flat mesh
/// may carry them on every face ([`pack_tagged`]), so each reader of a flat mesh takes them off.
pub const GPU_TAG_BITS: u32 = 0xff << 20;

/// A face of a flat mesh without its GPU tag (`GPU_TAG_BITS`).
fn untagged(f: &Face) -> Face {
    [f[0] & !GPU_TAG_BITS, f[1], f[2], f[3], f[4], f[5]]
}

/// Makes a flat mesh: x, z, sections, then per layer faces, starts, fluids, fluid
/// starts (each array preceded by its length in elements), then communications.
pub fn pack(m: &BlockMesh, out: &mut Vec<u32>) {
    pack_tagged(m, out, 0);
}

/// [`pack`] with the GPU tag `tag` (within `GPU_TAG_BITS`) set on every face: the faces of the flat
/// mesh are then those the GPU reads, layer by layer ([`packed_face_words`]), and the base of the
/// partial remeshes serves as GPU bytes too (`RUSTCRAFT_OPT_MESH_BASE_IS_PREPARED`).
pub fn pack_tagged(m: &BlockMesh, out: &mut Vec<u32>, tag: u32) {
    debug_assert_eq!(tag & !GPU_TAG_BITS, 0, "tag outside its bits");
    out.clear();
    out.extend([m.x as u32, m.z as u32, m.vis.len() as u32]);
    for l in 0..3 {
        out.push(m.layers[l].len() as u32);
        if tag == 0 {
            out.extend(m.layers[l].iter().flatten());
        } else {
            out.extend(m.layers[l].iter().flat_map(|f| [f[0] | tag, f[1], f[2], f[3], f[4], f[5]]));
        }
        out.push(m.starts[l].len() as u32);
        out.extend(&m.starts[l]);
        out.push(m.fluids[l].len() as u32);
        for q in &m.fluids[l] {
            out.extend(fluid_words(q));
        }
        out.push(m.fluid_starts[l].len() as u32);
        out.extend(&m.fluid_starts[l]);
    }
    out.extend(m.vis.iter().flat_map(|&v| [v as u32, (v >> 32) as u32]));
}

fn decode_fluid(q: &[u32; FLUID_WORDS]) -> crate::fluid::FluidQuad {
    crate::fluid::FluidQuad {
        corners: std::array::from_fn(|c| std::array::from_fn(|k| f32::from_bits(q[c * 5 + k]))),
        texture: q[20],
        colors: [q[21], q[22], q[23], q[24]],
        light: [q[25] as u16, (q[25] >> 16) as u16, q[26] as u16, (q[26] >> 16) as u16],
    }
}

/// Slices of a flat mesh ([`pack`]).
struct PackedView<'a> {
    x: i32,
    z: i32,
    sections: usize,
    /// Faces of each layer (words), and where they start in the flat mesh.
    faces: [&'a [u32]; 3],
    face_at: [usize; 3],
    starts: [&'a [u32]; 3],
    fluids: [&'a [u32]; 3],
    fluid_starts: [&'a [u32]; 3],
    vis: &'a [u32],
}

impl<'a> PackedView<'a> {
    fn new(w: &'a [u32]) -> Option<Self> {
        let mut at = 0;
        // Next `n` words, and where they start.
        let mut take = |n: usize| {
            let start = at;
            at += n;
            Some((w.get(start..at)?, start))
        };
        let head = take(3)?.0;
        let mut v = PackedView {
            x: head[0] as i32,
            z: head[1] as i32,
            sections: head[2] as usize,
            faces: [&[]; 3],
            face_at: [0; 3],
            starts: [&[]; 3],
            fluids: [&[]; 3],
            fluid_starts: [&[]; 3],
            vis: &[],
        };
        for l in 0..3 {
            let n = take(1)?.0[0] as usize;
            (v.faces[l], v.face_at[l]) = take(n * 6)?;
            let n = take(1)?.0[0] as usize;
            v.starts[l] = take(n)?.0;
            let n = take(1)?.0[0] as usize;
            v.fluids[l] = take(n * FLUID_WORDS)?.0;
            let n = take(1)?.0[0] as usize;
            v.fluid_starts[l] = take(n)?.0;
        }
        v.vis = take(v.sections * 2)?.0;
        Some(v)
    }
}

/// Word ranges of the faces of each layer in the flat mesh `w`: with [`pack_tagged`], the GPU bytes
/// of the faces, layer by layer.
pub fn packed_face_words(w: &[u32]) -> Option<[std::ops::Range<usize>; 3]> {
    let v = PackedView::new(w)?;
    Some(std::array::from_fn(|l| v.face_at[l]..v.face_at[l] + v.faces[l].len()))
}

/// Inverse of [`pack`] (checks); the faces without their GPU tag.
pub fn unpack(w: &[u32], version: u64) -> Option<BlockMesh> {
    let v = PackedView::new(w)?;
    Some(BlockMesh {
        x: v.x,
        z: v.z,
        version,
        layers: v.faces.map(|f| f.as_chunks::<6>().0.iter().map(untagged).collect()),
        fluids: v.fluids.map(|f| f.as_chunks::<FLUID_WORDS>().0.iter().map(decode_fluid).collect()),
        starts: v.starts.map(<[u32]>::to_vec),
        fluid_starts: v.fluid_starts.map(<[u32]>::to_vec),
        vis: v.vis.as_chunks::<2>().0.iter().map(|p| u64::from(p[0]) | u64::from(p[1]) << 32).collect(),
    })
}

/// Exact length (in words) of [`pack`]: the buffer is reserved in one go, with no copy or
/// growth.
pub fn packed_len(m: &BlockMesh) -> usize {
    let per_layer = (0..3)
        .map(|l| {
            4 + m.layers[l].len() * 6 + m.starts[l].len() + m.fluids[l].len() * FLUID_WORDS + m.fluid_starts[l].len()
        })
        .sum::<usize>();
    3 + per_layer + m.vis.len() * 2
}

/// [`pack`] into a new buffer of the exact size (base kept as is, no copy).
pub fn pack_exact(m: &BlockMesh) -> Vec<u32> {
    pack_exact_tagged(m, 0)
}

/// [`pack_tagged`] into a new buffer of the exact size.
pub fn pack_exact_tagged(m: &BlockMesh, tag: u32) -> Vec<u32> {
    let mut w = Vec::with_capacity(packed_len(m));
    pack_tagged(m, &mut w, tag);
    w
}

/// Words of a fluid face in [`pack`].
fn fluid_words(q: &crate::fluid::FluidQuad) -> [u32; FLUID_WORDS] {
    let mut w = [0u32; FLUID_WORDS];
    for (c, v) in q.corners.iter().flatten().enumerate() {
        w[c] = v.to_bits();
    }
    w[20] = q.texture;
    w[21..25].copy_from_slice(&q.colors);
    w[25] = u32::from(q.light[0]) | u32::from(q.light[1]) << 16;
    w[26] = u32::from(q.light[2]) | u32::from(q.light[3]) << 16;
    w
}

mod in_place;
pub use in_place::{InPlace, InPlacePaths, in_place_diff, in_place_diff_with};

/// Sorts the faces by group then by section (stable sort: the order within a same section and a
/// same group is kept, like that of a decal placed on its face). `starts` (S + 1 entries)
/// becomes `BUCKETS × (S + 1)` entries: start of group g, section s at `g × (S + 1) + s`.
fn group_faces_by_bucket(faces: &[Face], starts: &[u32], bucket: &[u8]) -> (Vec<Face>, Vec<u32>) {
    let sections_plus_one = starts.len();
    let group = |f: &Face| bucket.get(quad_of(f) as usize).map_or(BUCKETS - 1, |&b| b as usize);
    let mut count = vec![0u32; BUCKETS * sections_plus_one];
    for s in 0..sections_plus_one.saturating_sub(1) {
        for f in &faces[starts[s] as usize..starts[s + 1] as usize] {
            count[group(f) * sections_plus_one + s] += 1;
        }
    }
    let mut at = 0;
    let mut out_starts = vec![0u32; BUCKETS * sections_plus_one];
    for (o, c) in out_starts.iter_mut().zip(&count) {
        *o = at;
        at += c;
    }
    let mut next = out_starts.clone();
    let mut out = vec![[0u32; 6]; faces.len()];
    for s in 0..sections_plus_one.saturating_sub(1) {
        for f in &faces[starts[s] as usize..starts[s + 1] as usize] {
            let k = group(f) * sections_plus_one + s;
            out[next[k] as usize] = *f;
            next[k] += 1;
        }
    }
    (out, out_starts)
}

#[cfg(test)]
mod face_group_tests {
    use super::*;

    struct Rng(u64);

    impl Rng {
        fn next(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n.max(1)
        }
    }

    /// Faces collected for a random column through `ColumnFaces`: fresh faces and whole groups
    /// copied from a previous mesh, mixed in each section, unknown baked faces included.
    fn collect(by_group: bool, seed: u64, bucket: &[u8], kept: FaceGroups) -> ([Vec<Face>; 3], [Vec<u32>; 3], FaceGroups) {
        let mut r = Rng(seed);
        let sections = r.next(25) as usize;
        let mut faces = ColumnFaces::new(by_group, sections, kept);
        for s in 0..sections {
            faces.begin_section(s);
            for _ in 0..r.next(300) {
                let layer = r.next(3) as usize;
                // A few baked faces beyond the table: last group, like `group_faces_by_bucket`.
                let quad = r.next(bucket.len() as u64 + 3) as u32;
                let face = [s as u32, quad, r.next(1 << 30) as u32, 7, 8, layer as u32];
                if r.next(5) == 0 {
                    let g = face_group(bucket, quad);
                    let run: Vec<Face> = (0..r.next(4)).map(|k| [s as u32, quad, k as u32, 1, 2, 3]).collect();
                    faces.extend_group(layer, g, &run);
                } else {
                    faces.push(layer, face_group(bucket, quad), face);
                }
            }
        }
        faces.finish(bucket)
    }

    /// Faces pushed straight into their group give the faces and starts of the old sort at the
    /// end, on random columns, with the thread's lists reused from one column to the next; and the
    /// kept lists come back empty, within the kept capacity.
    #[test]
    fn faces_by_group_match_the_sort_at_the_end() {
        let mut r = Rng(0x2545_f491_4f6c_dd1d);
        let bucket: Vec<u8> = (0..500).map(|_| r.next(BUCKETS as u64) as u8).collect();
        let mut kept = FaceGroups::default();
        let mut faces = 0;
        for seed in 1..400u64 {
            let (old_layers, old_starts, _) = collect(false, seed, &bucket, FaceGroups::default());
            let (layers, starts, back) = collect(true, seed, &bucket, kept);
            assert_eq!(layers, old_layers, "column {seed}: faces differ");
            assert_eq!(starts, old_starts, "column {seed}: starts differ");
            assert!(back.iter().flatten().all(Vec::is_empty), "column {seed}: lists not emptied");
            kept = back;
            faces += layers.iter().map(Vec::len).sum::<usize>();
        }
        assert!(faces > 500_000, "draw too poor: {faces}");
        // A huge group is not kept whole by the thread.
        let mut big = ColumnFaces::new(true, 1, FaceGroups::default());
        big.begin_section(0);
        (0..3 * KEPT_FACES_PER_GROUP).for_each(|_| big.push(1, 4, [0; 6]));
        let (layers, _, back) = big.finish(&bucket);
        assert_eq!(layers[1].len(), 3 * KEPT_FACES_PER_GROUP);
        assert!(back[1][4].capacity() <= KEPT_FACES_PER_GROUP, "huge list kept: {}", back[1][4].capacity());
    }

    /// The comparison sees a face put in the wrong group (sabotage of the collection).
    #[test]
    fn face_in_the_wrong_group_is_seen() {
        let bucket = vec![0u8, 1, 2];
        let run = |wrong: bool| {
            let mut faces = ColumnFaces::new(true, 2, FaceGroups::default());
            let mut old = ColumnFaces::new(false, 2, FaceGroups::default());
            for s in 0..2 {
                faces.begin_section(s);
                old.begin_section(s);
                for q in 0..3u32 {
                    let face = [s as u32, q, 0, 0, 0, 0];
                    let g = face_group(&bucket, q);
                    faces.push(0, if wrong && q == 1 { 2 } else { g }, face);
                    old.push(0, g, face);
                }
            }
            let (a, b) = (faces.finish(&bucket), old.finish(&bucket));
            a.0 == b.0 && a.1 == b.1
        };
        assert!(run(false));
        assert!(!run(true), "face in the wrong group not seen");
    }
}

#[cfg(test)]
mod biome_tests {
    use super::*;

    /// Hashed seed of the world seed 12345 (reference obtained with the official server).
    const SEED: i64 = 293_737_985_876_514_017;

    #[test]
    fn zoom_matches_official() {
        // Quarts returned by the official 26.3 server's zoom for these blocks.
        for (p, q) in [
            ((0, 64, 0), (-1, 16, -1)),
            ((13, 70, -7), (2, 17, -3)),
            ((-101, -60, 255), (-26, -15, 63)),
            ((1000, 5, -2000), (250, 1, -500)),
            ((7, -64, 9), (1, -17, 2)),
            ((-3, 319, 2), (-2, 79, 0)),
        ] {
            assert_eq!(biome_zoom(SEED, p.0, p.1, p.2), q, "block {p:?}");
            assert_eq!(biome_zoom_cached(SEED, p.0, p.1, p.2), q, "block {p:?} (cache)");
        }
    }

    #[test]
    fn swamp_noise_matches_official() {
        // Biome information noise values (single precision) of the game.
        for ((x, z), v) in [
            ((0, 0), 0.0f32),
            ((100, -50), 0.026_717_55),
            ((-333, 777), 0.385_374_6),
            ((5000, 12), 0.538_654_2),
            ((-20, -20), 0.423_485_22),
            ((21, 3), -0.155_497_82),
        ] {
            let n = biome_info_noise(f64::from(x) * 0.0225, f64::from(z) * 0.0225);
            assert_eq!(n as f32, v, "column ({x}, {z})");
        }
        assert_eq!(swamp_grass(21, 3), SWAMP_GRASS_COLD);
        assert_eq!(swamp_grass(100, -50), SWAMP_GRASS_WARM);
    }

    fn column(x: i32, z: i32, biome: u32) -> Arc<Column> {
        Arc::new(Column {
            x,
            z,
            sections: (0..4).map(|_| rc_world::Section::empty(0, biome)).collect(),
            sky_light: vec![None; 6].into_boxed_slice(),
            block_light: vec![None; 6].into_boxed_slice(),
            heightmaps: Vec::new(),
        })
    }

    #[test]
    fn border_is_jagged_not_aligned_on_cells() {
        // Central column biome 0, east column biome 1: without zoom the border falls exactly at
        // x = 16; with the zoom, some blocks on either side take the neighbouring biome.
        let center = column(0, 0, 0);
        let east = column(1, 0, 1);
        let mut job = MeshJob::gather(center, 0, |x, z| (x == 1 && z == 0).then(|| east.clone()));
        job.biome_zoom_seed = SEED;
        job.min_y = -64;
        job.opts.blend = 0;
        let palette: BiomePalette = vec![[0x11_1111; 4], [0x22_2222; 4]];
        let mut crossed = 0;
        // Rows far from the absent north and south columns (otherwise fallback to the raw cell).
        for z in 4..12 {
            for x in 12..20 {
                let c = blended(&job, &palette, 0, x, 70, z);
                let wq = biome_zoom(SEED, x, 70 - 64, z).0;
                assert_eq!(c, if wq >= 4 { 0x22_2222 } else { 0x11_1111 }, "block ({x}, {z})");
                crossed += usize::from((c == 0x22_2222) != (x >= 16));
            }
        }
        assert!(crossed > 0, "border stayed aligned on the cells");
        // Absent column (north): plains, never biome 0.
        job.plains_biome = 1;
        job.opts.blend = 0;
        assert_eq!(blended(&job, &palette, 0, 8, 70, -6), 0x22_2222);
        // The blend equals the naive average without cache.
        job.opts.blend = 2;
        assert!(blend_matches_reference(&job, &palette));
    }

    /// Column of 4 sections (bottom of the world −64) whose every quart takes `pick(qx, qy, qz)` (absolute).
    fn mixed(x: i32, z: i32, pick: &dyn Fn(i32, i32, i32) -> u32) -> Arc<Column> {
        let sections = (0..4)
            .map(|s| {
                let mut sec = rc_world::Section::empty(0, 0);
                let v: Vec<u32> =
                    (0..64).map(|i| pick(x * 4 + (i & 3), -16 + s * 4 + (i >> 4), z * 4 + ((i >> 2) & 3))).collect();
                sec.biomes = rc_world::Container::pack(&v, 1, 3, 7);
                sec
            })
            .collect();
        Arc::new(Column {
            x,
            z,
            sections,
            sky_light: vec![None; 6].into_boxed_slice(),
            block_light: vec![None; 6].into_boxed_slice(),
            heightmaps: Vec::new(),
        })
    }

    /// Test palette: distinct colours per kind, swamp grass (biome 2), high bits on
    /// the water of biome 1; biome 9 outside the palette (white).
    fn test_palette() -> BiomePalette {
        vec![
            [0x11_2233, 0x44_5566, 0x77_8899, 0xaa_bbcc],
            [0x0f_1e2d, 0x3c_4b5a, 0x69_788f, 0xff3f_76e4],
            [SWAMP_GRASS, 0x6a_7039, 0x4c_763c, 0x61_7b64],
            [0xff_ffff, 0, 0x80_8080, 0x01_0203],
        ]
    }

    /// Test tasks: mixed biomes (north column absent, west column swamp), a single
    /// biome everywhere, swamp everywhere; centre at (2, −1) for negative coordinates.
    fn tint_jobs() -> Vec<(&'static str, MeshJob)> {
        let pick = |qx: i32, qy: i32, qz: i32| match (qx.div_euclid(3) + qz.div_euclid(2) + qy.div_euclid(5)).rem_euclid(5) {
            4 => 9,
            b => b as u32,
        };
        let (cx, cz) = (2, -1);
        let make = |f: &dyn Fn(i32, i32) -> Option<Arc<Column>>| {
            let mut j = MeshJob::gather(f(cx, cz).expect("centre"), 0, f);
            j.biome_zoom_seed = SEED;
            j.min_y = -64;
            j.plains_biome = 1;
            j
        };
        let mixed_job = make(&|x, z| match (x - cx, z - cz) {
            (0, -1) => None,
            (-1, _) => Some(mixed(x, z, &|_, _, _| 2)),
            _ => Some(mixed(x, z, &pick)),
        });
        let uniform = make(&|x, z| Some(mixed(x, z, &|_, _, _| 1)));
        let swamp = make(&|x, z| Some(mixed(x, z, &|_, _, _| 2)));
        vec![("mixed", mixed_job), ("one biome", uniform), ("swamp", swamp)]
    }

    /// Paths proven: all, none, each alone, all except one, sliding sums from the 1st request.
    fn tint_variants() -> Vec<TintPaths> {
        let (all, old) = (TintPaths::ALL, TintPaths::OLD);
        let mut v = vec![all, old, TintPaths { boxsum: 1, ..all }, TintPaths { boxsum: 1, ..old }];
        v.extend([
            TintPaths { uni: true, ..old },
            TintPaths { lattice: true, ..old },
            TintPaths { grid: true, ..old },
            TintPaths { idx: true, ..old },
            TintPaths { uni: false, ..all },
            TintPaths { lattice: false, ..all },
            TintPaths { grid: false, ..all },
            TintPaths { idx: false, ..all },
            TintPaths { boxsum: 0, ..all },
        ]);
        v
    }

    #[test]
    fn tint_paths_match_reference() {
        let palette = test_palette();
        for (name, mut job) in tint_jobs() {
            for rad in 0..=7 {
                job.opts.blend = rad;
                // Neighbouring column: direct access or search, alternating.
                job.tint.idx = rad % 2 == 0;
                if let Some(e) = tint_matches_reference(&job, &palette, &tint_variants(), 5) {
                    panic!("{name}, radius {rad}: {e}");
                }
            }
        }
    }

    #[test]
    fn tint_sabotage_is_caught() {
        let palette = test_palette();
        let (_, mut job) = tint_jobs().swap_remove(0);
        job.opts.blend = 2;
        assert_eq!(tint_matches_reference(&job, &palette, &[TintPaths::ALL], 5), None);
        let grid = TintPaths { grid: true, ..TintPaths::OLD };
        let boxes = TintPaths { boxsum: 1, ..TintPaths::ALL };
        let lattice = TintPaths { lattice: true, ..TintPaths::OLD };
        let (_, mut swamp) = tint_jobs().swap_remove(2);
        swamp.opts.blend = 2;
        for (mode, paths, j) in
            [(1, grid, &job), (2, TintPaths::ALL, &job), (3, boxes, &job), (4, TintPaths::ALL, &swamp), (5, lattice, &job)]
        {
            assert_eq!(tint_matches_reference(j, &palette, &[paths], 5), None, "without sabotage {mode}");
            SABOTAGE.set(mode);
            let r = tint_matches_reference(j, &palette, &[paths], 5);
            SABOTAGE.set(0);
            assert!(r.is_some(), "sabotage {mode} not seen");
        }
    }

    #[test]
    fn locate_direct_matches_search() {
        let (_, mut job) = tint_jobs().swap_remove(0);
        for z in -20..40 {
            for x in -20..40 {
                job.tint.idx = false;
                let a = locate(&job, x, z).map(|(c, lx, lz)| (c.x, c.z, lx, lz));
                job.tint.idx = true;
                let b = locate(&job, x, z).map(|(c, lx, lz)| (c.x, c.z, lx, lz));
                assert_eq!(a, b, "({x}, {z})");
            }
        }
    }
}

#[cfg(test)]
mod candidate_tests {
    use super::*;

    /// Property grid of random words, biased towards air and fluids so that every exit of the
    /// candidate test is taken.
    fn random_grid(seed: u64) -> Vec<u32> {
        use crate::cull::{AIR_BIT, FLUID_BIT};
        let mut x = seed;
        (0..P3)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let w = x as u32;
                match (x >> 40) % 8 {
                    0 => w | AIR_BIT,
                    1 => (w | FLUID_BIT) & !AIR_BIT,
                    2 => !AIR_BIT,
                    _ => w & !AIR_BIT,
                }
            })
            .collect()
    }

    /// The candidates and surely hidden faces of the lanes version (and of the AVX2 one when the
    /// processor has it) equal the cell-by-cell computation.
    #[test]
    fn lane_candidates_match_cell_by_cell() {
        for seed in [1, 0x9e37_79b9_7f4a_7c15, 0x2545_f491_4f6c_dd1d, 77] {
            let grid = random_grid(seed);
            let mut want = Vec::new();
            row_candidates(&grid, &mut want);
            let want_sure: Vec<u8> = want
                .iter()
                .map(|&i| {
                    let c = pad_index(usize::from(i));
                    crate::cull::sure_hidden(grid[c], neighbor_props(&grid, c)) as u8
                })
                .collect();
            let (mut got, mut sure) = (Vec::new(), Vec::new());
            row_candidates_by_lanes(&grid, &mut got, &mut sure);
            assert_eq!(got, want, "lanes, seed {seed}");
            assert_eq!(sure, want_sure, "lanes, seed {seed}");
            row_candidates_with_sure(&grid, &mut got, &mut sure);
            assert_eq!(got, want, "dispatched, seed {seed}");
            assert_eq!(sure, want_sure, "dispatched, seed {seed}");
            assert!(!want.is_empty() && want.len() < SECTION_BLOCKS, "seed {seed}: {} candidates", want.len());
        }
    }

    /// The solid render mask read from the grid equals the one of the states.
    #[test]
    fn solid_mask_of_props_matches_states() {
        let grid = random_grid(5);
        let mut want = [0u64; 64];
        for i in 0..SECTION_BLOCKS {
            if grid[pad_index(i)] & crate::cull::SOLID_RENDER_BIT != 0 {
                want[i >> 6] |= 1 << (i & 63);
            }
        }
        assert_eq!(solid_mask_of_props(&grid), want);
        assert!(want.iter().any(|&w| w != 0) && want.iter().any(|&w| w != u64::MAX));
    }
}
