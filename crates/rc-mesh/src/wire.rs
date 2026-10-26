//! Meshing task of a column (neighbours, settings, partial remeshing) and reach of a
//! change. The name comes from the wireframe of milestone 1, removed (the meshing is in `blocks`).

use std::sync::Arc;

use rc_world::Column;

/// Offsets of the neighbouring columns: sides (−x, +x, −z, +z) then corners (tint blending).
pub const NEIGHBORS: [(i32, i32); 8] = [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)];

/// Column to mesh and its neighbours in the order of [`NEIGHBORS`].
pub struct MeshJob {
    pub center: Arc<Column>,
    pub neighbors: [Option<Arc<Column>>; 8],
    pub version: u64,
    /// Shading per direction of the dimension (down, up, north, south, west, east).
    pub shade: [f32; 6],
    /// The dimension has sky light (otherwise sky at 0 everywhere).
    pub has_sky: bool,
    /// Video settings that change the meshing.
    pub opts: MeshOptions,
    /// Hashed seed of the dimension (login and respawn packets): blurred zoom of the biomes.
    pub biome_zoom_seed: i64,
    /// Ordinate of the bottom of the world (the meshing's y are relative to it; the zoom wants absolute y).
    pub min_y: i32,
    /// Biome of the absent columns (the game's empty level returns plains).
    pub plains_biome: u32,
    /// Partial remeshing: only the sections of the set are meshed, the others are
    /// taken as is from the previous mesh of the column (same world around them,
    /// hence same faces). `None`: the whole column.
    pub reuse: Option<(Sections, crate::blocks::Packed)>,
    /// Paths of the biome tint computation (same colours; `RUSTCRAFT_OPT_TINT*` switches
    /// read at startup, set here to compare the paths in one process).
    pub tint: crate::blocks::TintPaths,
    /// Smoothed light paths (same output; `RUSTCRAFT_OPT_AO*` switches read at startup).
    pub light: crate::light::LightPaths,
    /// Paths of the per-block loop and face removal rule (switches
    /// `RUSTCRAFT_OPT_MESH*`, `RUSTCRAFT_OPT_SHAPECULL` read at startup).
    pub cull: crate::cull::CullPaths,
}

/// Meshes that read changed cells of section `sy` of a column (edges `edges`,
/// `rc_world::edge`): column offsets in x and z, and sections of those columns. The meshing
/// of a block reads the light at one block and the states at two at most (state edges:
/// `edge::near_of_index`, within two cells of the edge; light: `edge::of_index`); for the light
/// (`light`), a section without sky data also reads the bottom row of the first sheet
/// above that has some (`sky_data(i)`: sheet of index i of the changed column has data,
/// i = section + 1). `sy` is −1 or `sections` for the sheets outside the world. `None`: none.
pub fn reach(
    edges: u8,
    sy: i32,
    sections: i32,
    light: bool,
    sky_data: impl Fn(i32) -> bool,
) -> Option<(Sections, std::ops::RangeInclusive<i32>, std::ops::RangeInclusive<i32>)> {
    use rc_world::edge;
    if edges & edge::ANY == 0 {
        return None;
    }
    let range = |lo: u8, hi: u8| -i32::from(edges & lo != 0)..=i32::from(edges & hi != 0);
    let ys = range(edge::Y0, edge::Y15);
    let (mut lo, hi) = (sy + ys.start(), sy + ys.end());
    if light && edges & edge::Y0 != 0 {
        // Sheets without data just below that of the section (index sy + 1): they read its
        // bottom row; their cells (sections k ..= sy − 1) are read by sections k − 1 ..= sy.
        let mut k = sy;
        while k >= 0 && !sky_data(k) {
            k -= 1;
        }
        if k < sy {
            lo = lo.min(k - 1);
        }
    }
    let (lo, hi) = (lo.max(0), hi.min(sections - 1));
    if lo > hi {
        return None;
    }
    let mut mask = Sections::default();
    for s in lo..=hi {
        mask.insert(s);
    }
    Some((mask, range(edge::X0, edge::X15), range(edge::Z0, edge::Z15)))
}

/// Set of sections of a column (256 at most: maximum world height 4,064 blocks).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sections([u64; 4]);

impl Sections {
    pub const ALL: Sections = Sections([u64::MAX; 4]);

    /// Adds section `s` (ignored outside 0..256).
    pub fn insert(&mut self, s: i32) {
        if (0..256).contains(&s) {
            self.0[(s >> 6) as usize] |= 1 << (s & 63);
        }
    }

    pub fn contains(&self, s: usize) -> bool {
        s < 256 && self.0[s >> 6] >> (s & 63) & 1 == 1
    }

    pub fn union(&mut self, o: Sections) {
        for (a, b) in self.0.iter_mut().zip(o.0) {
            *a |= b;
        }
    }

    pub fn is_all(&self) -> bool {
        *self == Self::ALL
    }

    pub fn is_empty(&self) -> bool {
        self.0 == [0; 4]
    }

    /// Number of sections among the first `n`.
    pub fn count(&self, n: usize) -> usize {
        (0..n.min(256)).filter(|&s| self.contains(s)).count()
    }
}

/// Video settings read by the meshing (`option-menus.md` § 3.5); a change remeshes everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshOptions {
    /// `biomeBlendRadius` (0..=7): blending of biome colours over (2r + 1)² blocks.
    pub blend: i32,
    /// `ao`: smoothed light and ambient occlusion; otherwise flat light.
    pub ao: bool,
    /// `cutoutLeaves`: cutout leaves; otherwise opaque (solid layer) and faces between
    /// leaves removed.
    pub cutout_leaves: bool,
    /// Format (not a game setting, same image): offset models (grass, flowers…) as compact
    /// 24 B faces instead of explicit quads (`RUSTCRAFT_OPT_OFFSETFACES`).
    pub offset_faces: bool,
    /// Format: back face of the water carried by a quad bit instead of a second quad
    /// (`RUSTCRAFT_OPT_WATERDOUBLE`).
    pub water_double: bool,
}

impl Default for MeshOptions {
    fn default() -> Self {
        MeshOptions { blend: 2, ao: true, cutout_leaves: true, offset_faces: true, water_double: true }
    }
}

/// Default directional shading, and that of the dimensions with "nether" lighting.
pub const SHADE_DEFAULT: [f32; 6] = [0.5, 1.0, 0.8, 0.8, 0.6, 0.6];
pub const SHADE_NETHER: [f32; 6] = [0.9, 0.9, 0.8, 0.8, 0.6, 0.6];

impl MeshJob {
    /// Builds the task from access to the loaded columns.
    pub fn gather(center: Arc<Column>, version: u64, get: impl Fn(i32, i32) -> Option<Arc<Column>>) -> Self {
        let (x, z) = (center.x, center.z);
        MeshJob {
            neighbors: NEIGHBORS.map(|(dx, dz)| get(x + dx, z + dz)),
            center,
            version,
            shade: SHADE_DEFAULT,
            has_sky: true,
            opts: MeshOptions::default(),
            biome_zoom_seed: 0,
            min_y: 0,
            plains_biome: 0,
            reuse: None,
            tint: crate::blocks::TintPaths::from_env(),
            light: crate::light::LightPaths::from_env(),
            cull: crate::cull::CullPaths::from_env(),
        }
    }
}
