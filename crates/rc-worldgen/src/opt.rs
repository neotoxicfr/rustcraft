//! Optimisation switches of the generator, read once (first use) from
//! the environment: absent or any value other than `0` = new path, `0` = old path.
//! The equivalence tests call both paths explicitly, without going through here.
//! Dependencies: `NOISE_COLCONST`, `NOISE_SIMD`, `NOISE_AVX2`, `NOISE_XPRE` and
//! `NOISE_XY_PERM_TABLE` only act in the precomputed path (`NOISE_YPRE`), and `NOISE_AVX2` only
//! with `NOISE_SIMD`; `NOISE_BATCH_AVX2` only acts on the batches of `NOISE_BATCH_POINTS` with
//! `PERM512`, and `NOISE_BATCH_SHARED_CELL` only inside the AVX2 batch (`NOISE_BATCH_AVX2`, so
//! not at all on a processor without AVX2). To isolate a track
//! on the scene bench: ypre = YPRE=1 COLCONST=0 SIMD=0 against YPRE=0; colconst, simd = the single
//! track at 0; avx2 = AVX2=0 against all. Batches: points = BATCH_POINTS=1 BATCH_AVX2=0 against
//! BATCH_POINTS=0; batch avx2 = BATCH_SHARED_CELL=0 against BATCH_AVX2=0; shared cell = the single
//! track at 0.

use std::sync::LazyLock;

fn on(names: &[&str]) -> bool {
    !names.iter().any(|n| rc_data::env_flags::read_os(n).and_then(|v| v.into_string().ok()).is_some_and(|v| v.trim() == "0"))
}

macro_rules! switch {
    ($(#[$m:meta])* $f:ident, $($name:literal),+) => {
        $(#[$m])*
        #[inline]
        pub fn $f() -> bool {
            static S: LazyLock<bool> = LazyLock::new(|| on(&[$($name),+]));
            *S
        }
    };
}

switch!(
    /// `floor` the Java way (truncation then correction) instead of the library call.
    floor_java,
    "RUSTCRAFT_OPT_FLOOR_JAVA"
);
switch!(
    /// Noise volume: work in y computed once per octave, not per column.
    noise_ypre,
    "RUSTCRAFT_OPT_NOISE_YPRE"
);
switch!(
    /// Noise volume: column whose y inputs are all equal computed once.
    noise_colconst,
    "RUSTCRAFT_OPT_NOISE_COLCONST"
);
switch!(
    /// Perlin: doubled permutation table (512) and gradients already in f32.
    perm512,
    "RUSTCRAFT_OPT_PERM512",
    "RUSTCRAFT_OPT_NOISE_PERM512"
);
switch!(
    /// Noise volume: runs of the same cell in y through a branchless kernel (vectorised).
    noise_simd,
    "RUSTCRAFT_OPT_NOISE_SIMD"
);
switch!(
    /// Noise volume: the work in x (cell, position in the cell, smoothing, first permutations)
    /// computed once per octave for the columns of a row, not again for every row in z.
    noise_x_precomputed,
    "RUSTCRAFT_OPT_NOISE_XPRE"
);
switch!(
    /// Noise volume: the 4 permutations that depend on x and y only, kept per x cell and run of
    /// the same y cell, read again by every column in z of that x cell.
    noise_xy_perm_table,
    "RUSTCRAFT_OPT_NOISE_XY_PERM_TABLE"
);
switch!(
    /// Shifted noise volumes: the points of the volume handed to the noise in batches (layers
    /// outside, points inside) instead of the whole noise point by point.
    noise_batch_points,
    "RUSTCRAFT_OPT_NOISE_BATCH_POINTS"
);
switch!(
    /// AVX2 batches of noise points: a group of 8 points in one cell (low octaves) reads its
    /// permutations once instead of gathering them for each point.
    noise_batch_shared_cell,
    "RUSTCRAFT_OPT_NOISE_BATCH_SHARED_CELL"
);
switch!(
    /// `sincos`: a single argument reduction and a single pair of series.
    sincos,
    "RUSTCRAFT_OPT_SINCOS"
);
switch!(
    /// `jmath` products: exact error through Veltkamp's split instead of the library `fma` call
    /// (used when the FMA instruction is absent or `RUSTCRAFT_OPT_TWO_PROD_FMA=0`).
    two_prod_split,
    "RUSTCRAFT_OPT_TWO_PROD_SPLIT"
);
switch!(
    /// `jmath` products: bodies compiled for the FMA instruction, if the processor has it.
    /// Both `TWO_PROD` switches at 0 restore the library call.
    two_prod_fma,
    "RUSTCRAFT_OPT_TWO_PROD_FMA"
);
switch!(
    /// `jmath` sin or cos alone: only the Taylor series it returns, not both.
    sincos_only_needed_series,
    "RUSTCRAFT_OPT_SINCOS_ONE_SERIES"
);
switch!(
    /// `jmath`: when both Taylor series are needed, they advance in the same loop.
    sincos_interleaved_series,
    "RUSTCRAFT_OPT_SINCOS_INTERLEAVED"
);

switch!(
    /// Volume evaluator: choices (range, interval, lerp) that compute each branch only on
    /// the y rows where it is used.
    range_yvol,
    "RUSTCRAFT_OPT_RANGE_YVOL"
);

switch!(
    /// Evaluator: a buffer taken from the context keeps its old values instead of being zeroed
    /// (every caller writes the elements it reads).
    take_without_zeroing,
    "RUSTCRAFT_OPT_TAKE_WITHOUT_ZEROING"
);
switch!(
    /// Evaluator: a buffer is taken from the smallest spare one that holds it, a new one is
    /// allocated at a rounded size, and the spare buffers of a context are capped in bytes
    /// (8 MB) instead of in count.
    best_fit_buffers,
    "RUSTCRAFT_OPT_BEST_FIT_BUFFERS"
);
switch!(
    /// Evaluator: a cache whose input is exact (point and volume give the same bits everywhere
    /// below) serves any volume whose points it already holds, for instance the corners of the
    /// final density inside the quart grid the aquifer filled first.
    sublattice_cache_read,
    "RUSTCRAFT_OPT_SUBLATTICE_CACHE_READ"
);
switch!(
    /// Evaluator: a cache whose input never reads y has one point value per column; where a point
    /// read would compute it, the value known from another height answers (the last point, or for
    /// an exact input the buffer at y = 0).
    column_memo_y_independent,
    "RUSTCRAFT_OPT_COLUMN_MEMO_Y_INDEPENDENT"
);
switch!(
    /// Evaluator: a volume spline holds its coordinate buffers on the stack (up to 8
    /// coordinates) instead of in an allocated list.
    spline_inputs_on_stack,
    "RUSTCRAFT_OPT_SPLINE_INPUTS_ON_STACK"
);
switch!(
    /// Spline: point coordinates kept on the stack, sampling without dynamic call. On by default:
    /// point climate −0.6% without caches, −1.2% with caches, one allocation per quarter less
    /// (12 alternated rounds, loaded machine; docs/perf.md, density evaluator section); off before,
    /// judged on a volume bench where it has no work.
    spline_stack,
    "RUSTCRAFT_OPT_SPLINE_STACK"
);

switch!(
    /// Biome tree: candidate distance carried, early exit at zero distance.
    btree_early,
    "RUSTCRAFT_OPT_BTREE_EARLY"
);
switch!(
    /// Flattened biome tree (contiguous children, i32 bounds as a structure of arrays).
    btree_soa,
    "RUSTCRAFT_OPT_BTREE_SOA",
    "RUSTCRAFT_OPT_BIOMETREE_FLAT"
);
/// Biome tree: memo of the last climate (identical target and last leaf). Off by
/// default (`=1` enables it): 96 repeated targets out of 1.57 million on the micro-bench, no gain.
#[inline]
pub fn btree_memo() -> bool {
    static S: LazyLock<bool> = LazyLock::new(|| {
        rc_data::env_flags::read_os("RUSTCRAFT_OPT_BTREE_MEMO").and_then(|v| v.into_string().ok()).is_some_and(|v| v.trim() == "1")
    });
    *S
}
/// Biome search over a sequence of points (`/locate biome`): stateless climate and leaf
/// computed in parallel batches, last leaf then applied in series (×8.9 at 32 threads, same
/// biome as one point after the other). On by default: even with the global pool kept busy by a
/// stand-in for chunk generation, its batches still beat the series (28.3 ms against 77.6).
#[inline]
pub fn btree_split() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_BTREE_SPLIT")
}
switch!(
    /// With `btree_split`: stateless search bounded by the distance of the neighbouring point's leaf
    /// (+ 1), instead of starting unbounded.
    btree_split_hint,
    "RUSTCRAFT_OPT_BTREE_SPLIT_HINT"
);
switch!(
    /// With `btree_split_hint`: the neighbouring leaf serves as candidate (fall back to the chained
    /// search if it stays the best and the last leaf is not at dmin); `=0`:
    /// bound d + 1 without candidate.
    btree_split_cand,
    "RUSTCRAFT_OPT_BTREE_SPLIT_CAND"
);
switch!(
    /// Biome zoom: memoised corner offsets (direct cache per corner).
    zoom_memo,
    "RUSTCRAFT_OPT_ZOOM_MEMO"
);

/// Flattened biome tree: distances of the children in AVX2, if the processor has it.
#[inline]
pub fn btree_simd() -> bool {
    static S: LazyLock<bool> = LazyLock::new(|| on(&["RUSTCRAFT_OPT_BTREE_SIMD"]) && rc_data::cpu::has_avx2());
    *S
}

switch!(
    /// Terrain: block identifiers (dirt, grass, mycelium, water, lava) resolved once per
    /// generator, and table lookup without allocation for an already full name.
    terrain_ids,
    "RUSTCRAFT_OPT_TERRAIN_IDS"
);
switch!(
    /// Aquifers, filling: window of the 12 neighbouring cells kept per column (x, z, y cell).
    aq_colwin,
    "RUSTCRAFT_OPT_AQ_COLWIN"
);
switch!(
    /// Aquifers, filling: 4 nearest through a branchless comparison network (with
    /// `AQ_COLWIN` only).
    aq_simd,
    "RUSTCRAFT_OPT_AQ_SIMD"
);
switch!(
    /// Filling: positive density (default block) tested before the call to the aquifer.
    aq_pretest,
    "RUSTCRAFT_OPT_AQ_PRETEST"
);
switch!(
    /// Base height: density of the column by slices aligned on the cells, from the top
    /// downwards, stopped at the first block that satisfies the predicate.
    baseh_lazy,
    "RUSTCRAFT_OPT_BASEH_LAZY"
);
switch!(
    /// Base height: memo shared per generator, key (x, z, type).
    baseh_memo,
    "RUSTCRAFT_OPT_BASEH_MEMO"
);
switch!(
    /// Carved caves: normalised vertical difference computed once per y and per ellipsoid.
    carver_yd,
    "RUSTCRAFT_OPT_CARVER_YD"
);
switch!(
    /// Canyons: width buffer reused instead of one allocation per canyon.
    carver_widths,
    "RUSTCRAFT_OPT_CARVER_WIDTHS"
);
switch!(
    /// Carved caves: carving of each source recorded once then replayed per target chunk.
    carver_replay,
    "RUSTCRAFT_OPT_CARVER_REPLAY"
);
/// Carved caves: a source whose starting draw fails is skipped before the replay cache (no lock,
/// no hashing, nothing stored for it).
#[inline]
pub fn carver_start_pretest() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_CARVER_START_PRETEST")
}
/// Carved caves: a recorded carving whose ellipsoids all miss the target chunk is not replayed.
#[inline]
pub fn carver_reach_box() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_CARVER_REACH_BOX")
}
/// Carved caves: the mask read back by runs, a word at a time, instead of bit by bit.
#[inline]
pub fn carving_mask_read_by_runs() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_CARVING_MASK_READ_BY_RUNS")
}
/// Biomes of a batch: cave biomes of the sources kept in a dense tile over the batch's rectangle
/// (one lookup per position instead of one per target and position).
#[inline]
pub fn batch_biome_tile() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_BATCH_BIOME_TILE")
}
/// Biomes of a batch: chunk biomes and cave biomes computed in one parallel pass, not two.
#[inline]
pub fn merged_biome_phase() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_MERGED_BIOME_PHASE")
}
/// Aquifers: the preliminary surface levels are computed once per tile of 4 × 4 chunks when a
/// batch is prepared and copied by the aquifers of its chunks, instead of each aquifer computing
/// the 11 × 11 quarts around its chunk (its neighbours computing most of them again).
#[inline]
pub fn aquifer_surface_tiles() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_AQUIFER_SURFACE_TILES")
}
/// Aquifers (with the surface tiles): a tile keeps the 2D climate its levels were computed from
/// (offset, factor and the noises under them), and the context of each chunk starts with it, as
/// after a window computed by the chunk: the final density does not compute the climate of its
/// corners again.
#[inline]
pub fn shared_climate_tiles() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_SHARED_CLIMATE_TILES")
}
/// Filling: each block written without reading the old one back, and the two generation height
/// maps of a column worked out while walking it down, written once at its end.
#[inline]
pub fn fill_column_tracking() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_FILL_COLUMN_TRACKING")
}
/// Terrain to decoration chunk: a section of plain air is not converted state by state, and the
/// highest other section bounds the walk of the final height maps.
#[inline]
pub fn skip_air_sections() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_SKIP_AIR_SECTIONS")
}
/// Terrain to decoration chunk: converted layer by layer from the top, the four final height
/// maps found on the way instead of a second walk.
#[inline]
pub fn convert_with_heightmaps() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_CONVERT_WITH_HEIGHTMAPS")
}
/// Terrain to decoration chunk: the block buffer of the converted proto-chunk is kept by its
/// thread for the next proto-chunk instead of being freed.
#[inline]
pub fn reuse_proto_buffer() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_REUSE_PROTO_BUFFER")
}
switch!(
    /// Volume: index of a block by shift and mask when the steps are powers of 2.
    vol_pow2,
    "RUSTCRAFT_OPT_VOL_POW2"
);
switch!(
    /// Materials: index of the block in the vein volume drawn from a column base.
    ore_idx,
    "RUSTCRAFT_OPT_ORE_IDX"
);
switch!(
    /// Materials: vein skipped on the y rows where its density is surely a constant ≤ 0.
    ore_early,
    "RUSTCRAFT_OPT_ORE_EARLY"
);
switch!(
    /// Surface: root rule not evaluated on the rows where it surely returns "nothing".
    surf_skip,
    "RUSTCRAFT_OPT_SURF_SKIP"
);
switch!(
    /// Surface: rule tree specialised per y band (conditions decided by y removed).
    surf_bands,
    "RUSTCRAFT_OPT_SURF_BANDS"
);
switch!(
    /// Vein pre-filling: only the y rows where the density is not constant.
    vein_yrange,
    "RUSTCRAFT_OPT_VEIN_YRANGE"
);
switch!(
    /// Vein pre-filling: richness only on the rows where a density is positive.
    vein_lazyrich,
    "RUSTCRAFT_OPT_VEIN_LAZYRICH"
);
switch!(
    /// Vein pre-filling: a single y range for all the veins (shared caches).
    vein_shared,
    "RUSTCRAFT_OPT_VEIN_SHARED"
);
switch!(
    /// Materials: evaluation state and buffers reused per thread, lazy cave veins.
    mat_reuse,
    "RUSTCRAFT_OPT_MAT_REUSE"
);
switch!(
    /// Materials: column constants (bands, temperature) kept per column.
    mat_colcache,
    "RUSTCRAFT_OPT_MAT_COLCACHE"
);
switch!(
    /// Surface: under the preliminary surface, cells decided per y band by a stateless kernel
    /// (block, vein, vein then block, or the band's tree) instead of the column walk.
    surface_band_kernels,
    "RUSTCRAFT_OPT_SURFACE_BAND_KERNELS"
);
/// Surface: the block of the "single block" and "vein then block" bands under every column's
/// preliminary surface (bedrock floor, deepslate, the deepslate around the iron vein) written layer
/// by layer over the 256 columns before them, the columns then only evaluating the vein (with the
/// band kernels). Surface step −3 to −9 % (loaded-machine micro-bench, see `docs/perf.md`).
#[inline]
pub fn surface_row_kernel() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_SURFACE_ROW_KERNEL")
}
switch!(
    /// Surface: a plain solid replaced by a plain solid is written in place, without the height map
    /// and section count updates that cannot change.
    direct_solid_write,
    "RUSTCRAFT_OPT_DIRECT_SOLID_WRITE"
);
switch!(
    /// Vein pre-filling of the surface: rows above the highest solid of the chunk skipped.
    vein_top_bound,
    "RUSTCRAFT_OPT_VEIN_TOP_BOUND"
);
switch!(
    /// Cave top material: biomes read from the chunks already computed around (the source only
    /// outside them), instead of recomputing the climate at each call. Read by the caves step.
    cave_top_from_chunk_biomes,
    "RUSTCRAFT_OPT_CAVE_TOP_FROM_CHUNK_BIOMES"
);
switch!(
    /// Cave top material: surface depth and preliminary surface of each column kept from the
    /// surface step of the same chunk.
    cave_top_column_memo,
    "RUSTCRAFT_OPT_CAVE_TOP_COLUMN_MEMO"
);
switch!(
    /// Block tables: "same block" compared by block identifier numbered at interning, not by name.
    base_block_ids,
    "RUSTCRAFT_OPT_BASE_BLOCK_IDS"
);
switch!(
    /// Lookups by name (tags, placed features) without allocated string: full name borrowed,
    /// `minecraft:` prefix composed on the stack.
    tag_borrow,
    "RUSTCRAFT_OPT_TAG_BORROW"
);
switch!(
    /// Shape updates: families as block sets and properties resolved once, block
    /// outside any family sent directly to the decoration rules.
    shape_keys,
    "RUSTCRAFT_OPT_SHAPE_KEYS"
);
switch!(
    /// Position sets of the trees: buckets kept at clear, in-place doubling, cursor
    /// of the first non-empty bucket.
    jset_keep,
    "RUSTCRAFT_OPT_JSET_KEEP"
);
switch!(
    /// Tree decorators: lists copied into reused buffers, exclusion set
    /// kept, trunks poured without an intermediate list.
    tree_take,
    "RUSTCRAFT_OPT_TREE_TAKE"
);
switch!(
    /// Faces on the edge of a tree: shape update skipped for a block it never changes.
    tree_faceskip,
    "RUSTCRAFT_OPT_TREE_FACESKIP"
);
switch!(
    /// Icebergs: sine and cosine of the angle computed once per iceberg and per slice.
    berg_trig,
    "RUSTCRAFT_OPT_BERG_TRIG"
);
switch!(
    /// Veins: chunk of each column (x, z) resolved once per x in the sphere loop.
    ore_col,
    "RUSTCRAFT_OPT_ORE_COL"
);
switch!(
    /// Freezing and snow: noise-free temperature when the altitude noise bound proves the
    /// biome stays warm enough to rain.
    freeze_threshold,
    "RUSTCRAFT_OPT_FREEZE_THRESHOLD"
);
switch!(
    /// Surface freezing: frozen part and altitude noise computed once for y and y − 1.
    freeze_col,
    "RUSTCRAFT_OPT_FREEZE_COL"
);
switch!(
    /// Geodes: 1/√ terms of the first pass reused in the second.
    geode_reuse,
    "RUSTCRAFT_OPT_GEODE_REUSE"
);
switch!(
    /// Geodes: row in x skipped when its possible maximum stays under the outer crust.
    geode_rowskip,
    "RUSTCRAFT_OPT_GEODE_ROWSKIP"
);
switch!(
    /// Lookups by name of the features (bamboo, monster room, chorus, paths, shelf mushrooms,
    /// pale moss patch) resolved once.
    decor_ids,
    "RUSTCRAFT_OPT_DECOR_IDS"
);
switch!(
    /// Boolean properties and directions by precomputed value index, presence of a
    /// property by block bit mask.
    prop_idx,
    "RUSTCRAFT_OPT_PROP_IDX"
);

// ---- Templates and jigsaw (C1-M11). The proofs force a path on their thread through `force`, because
// these paths are buried under assembly and placement (no parameter to pass down).

#[cfg(test)]
thread_local! {
    static FORCED: std::cell::RefCell<Vec<(&'static str, bool)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Forces (tests) the value of the named switches on the current thread; `None` = environment.
#[cfg(test)]
pub(crate) fn force(names: &[&'static str], v: Option<bool>) {
    FORCED.with(|f| {
        let mut f = f.borrow_mut();
        f.retain(|(n, _)| !names.contains(n));
        if let Some(v) = v {
            f.extend(names.iter().map(|n| (*n, v)));
        }
    });
}

#[cfg(test)]
fn forced(name: &str) -> Option<bool> {
    FORCED.with(|f| f.borrow().iter().find(|(n, _)| *n == name).map(|(_, v)| *v))
}

/// Switches of C1-M11, in track order (names without the `RUSTCRAFT_OPT_` prefix).
#[cfg(test)]
pub(crate) const C1M11: [&str; 10] = [
    "TEMPL_TABLE",
    "ELEM_ONCE",
    "POOLS_TABLE",
    "JIGSAW_PRECALC",
    "JIGSAW_IDX",
    "PROCS_SHARED",
    "PROCS_TABLES",
    "PROCESS_BOX",
    "LOCKED_SET",
    "STATETRANS_FAST",
];

macro_rules! switch_f {
    ($(#[$m:meta])* $f:ident, $short:literal) => {
        $(#[$m])*
        #[inline]
        pub fn $f() -> bool {
            #[cfg(test)]
            if let Some(v) = forced($short) {
                return v;
            }
            static S: LazyLock<bool> = LazyLock::new(|| on(&[concat!("RUSTCRAFT_OPT_", $short)]));
            *S
        }
    };
}

switch_f!(
    /// Templates: table frozen at construction (bytes + single-fill cell per id),
    /// read without lock, borrowed key; a single decoding per template even with several threads.
    templ_table,
    "TEMPL_TABLE"
);
switch_f!(
    /// "Single template" element: template resolved once and kept in the element.
    elem_once,
    "ELEM_ONCE"
);
switch_f!(
    /// Pools, processor lists and maximum size of a pool: frozen tables with single
    /// cells, read without lock.
    pools_table,
    "POOLS_TABLE"
);
switch_f!(
    /// Assembly: junctions transformed and box precomputed per (element, rotation) at
    /// zero position; box of each trial deduced by displacement.
    jigsaw_precalc,
    "JIGSAW_PRECALC"
);
switch_f!(
    /// Assembly: candidates and junctions shuffled by indices (no copy of the elements).
    jigsaw_idx,
    "JIGSAW_IDX"
);

/// Switches of the jigsaw assembly work savings, in track order (names without the
/// `RUSTCRAFT_OPT_` prefix).
#[cfg(test)]
pub(crate) const JIGSAW_ASSEMBLY: [&str; 6] = [
    "JIGSAW_FAILED_ELEMENT_MEMO",
    "JIGSAW_EXPAND_MEMO",
    "JIGSAW_RECENT_FIRST",
    "JIGSAW_SPATIAL_GRID",
    "JIGSAW_REUSED_BUFFERS",
    "LAZY_POOLS",
];

switch_f!(
    /// Assembly: an element already tried at a junction is not tried again (it fails the same
    /// way); only its random draws are replayed.
    jigsaw_failed_element_memo,
    "JIGSAW_FAILED_ELEMENT_MEMO"
);
switch_f!(
    /// Assembly: expansion height of the village trick worked out once per (element, rotation)
    /// and start.
    jigsaw_expand_memo,
    "JIGSAW_EXPAND_MEMO"
);
/// Assembly: free space test walks the placed boxes from the most recent one, only
/// `RUSTCRAFT_OPT_JIGSAW_RECENT_FIRST=1` (off by default: a village compares 1.5 times as many
/// boxes that way, the boxes its trials meet being mostly the oldest ones; see `docs/perf.md`).
#[inline]
pub fn jigsaw_recent_first() -> bool {
    #[cfg(test)]
    if let Some(v) = forced("JIGSAW_RECENT_FIRST") {
        return v;
    }
    static S: LazyLock<bool> =
        LazyLock::new(|| rc_data::env_str!("RUSTCRAFT_OPT_JIGSAW_RECENT_FIRST").is_some_and(|v| v.trim() == "1"));
    *S
}
switch_f!(
    /// Assembly: placed boxes of a crowded free space indexed by 16-block columns.
    jigsaw_spatial_grid,
    "JIGSAW_SPATIAL_GRID"
);
switch_f!(
    /// Assembly: index buffers of the shuffles kept from one trial to the next.
    jigsaw_reused_buffers,
    "JIGSAW_REUSED_BUFFERS"
);
switch_f!(
    /// Pools: text of each pool parsed at its first lookup, not at generator start.
    lazy_pools,
    "LAZY_POOLS"
);
switch_f!(
    /// Placement: shared processor list (prefix, common list, suffix) instead of a copy
    /// per placed piece; inline processors of an element analysed once.
    procs_shared,
    "PROCS_SHARED"
);
switch_f!(
    /// Processors: marker blocks, blackstone table, ageing classes and
    /// loot containers resolved once per block.
    procs_tables,
    "PROCS_TABLES"
);
switch_f!(
    /// Placement: only the template blocks in the y slice of the area are walked.
    process_box,
    "PROCESS_BOX"
);
switch_f!(
    /// Placement: positions of the locked sources in a hashed set.
    locked_set,
    "LOCKED_SET"
);
switch_f!(
    /// State transformation tables: blocks without an oriented property copied as is.
    statetrans_fast,
    "STATETRANS_FAST"
);

// ---- State transformation tables (C1-M07). The proofs pass every combination explicitly.

switch!(
    /// State transformation tables: half and three-quarter turns composed from the quarter turn
    /// table; mirrored stairs read the half-turn table instead of turning again.
    compose_rotations,
    "RUSTCRAFT_OPT_COMPOSE_ROTATIONS"
);
switch!(
    /// State transformation tables: each block's property rules worked out once, then applied
    /// to its states by value index (no text rewrite per state).
    transforms_per_block,
    "RUSTCRAFT_OPT_TRANSFORMS_PER_BLOCK"
);
switch!(
    /// State transformation tables kept by the process once built: a world opened again reuses
    /// them although its generation data are rebuilt.
    keep_transforms,
    "RUSTCRAFT_OPT_KEEP_TRANSFORMS"
);

/// State transformation tables built across the rayon pool, only `RUSTCRAFT_OPT_PARALLEL_TRANSFORMS=1`
/// (off by default: the sequential build is already short, see `docs/perf.md`).
pub fn parallel_transforms() -> bool {
    static S: LazyLock<bool> =
        LazyLock::new(|| rc_data::env_str!("RUSTCRAFT_OPT_PARALLEL_TRANSFORMS").is_some_and(|v| v.trim() == "1"));
    *S
}

/// Where the state transformation tables of a world's templates are built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TablesThread {
    /// On the spot, before the generator is ready (old path).
    Inline,
    /// On a dedicated background thread.
    Dedicated,
    /// As a task of the rayon pool (which the stronghold rings occupy at the same time).
    RayonPool,
}

/// `RUSTCRAFT_OPT_TRANSFORMS_IN_BACKGROUND`: `0` inline, `rayon` pool task, otherwise a dedicated
/// thread.
pub fn transforms_in_background() -> TablesThread {
    static S: LazyLock<TablesThread> =
        LazyLock::new(|| match rc_data::env_str!("RUSTCRAFT_OPT_TRANSFORMS_IN_BACKGROUND").map(str::trim) {
            Some("0") => TablesThread::Inline,
            Some("rayon") => TablesThread::RayonPool,
            _ => TablesThread::Dedicated,
        });
    *S
}
switch_f!(
    /// Height maps of a generated chunk: every map from a single walk down each column, started
    /// at the top of the highest slice of 16 that is not all plain air (L9-STRUCT).
    prime_one_walk,
    "PRIME_ONE_WALK"
);

/// Vectorised kernel compiled for AVX2, if the processor has it (detection done once).
#[inline]
pub fn noise_avx2() -> bool {
    static S: LazyLock<bool> = LazyLock::new(|| on(&["RUSTCRAFT_OPT_NOISE_AVX2"]) && rc_data::cpu::has_avx2());
    *S
}

/// Batches of noise points: 8 points at a time in AVX2 registers (permutations read by gathers),
/// if the processor has it (detection done once).
#[inline]
pub fn noise_batch_avx2() -> bool {
    static S: LazyLock<bool> = LazyLock::new(|| on(&["RUSTCRAFT_OPT_NOISE_BATCH_AVX2"]) && rc_data::cpu::has_avx2());
    *S
}

switch_f!(
    /// Named density functions and noise parameters of a data pack parsed once and shared by every
    /// generator built from that pack, instead of once per generator (four or more per dimension).
    shared_generator_defs,
    "SHARED_GENERATOR_DEFS"
);
switch_f!(
    /// JSON reader: numbers and object keys stored in place, an object as one shared block of members
    /// sorted by key, instead of a string per number and per key and a tree map per object.
    compact_json,
    "COMPACT_JSON"
);
switch_f!(
    /// A data pack file parsed once and its tree shared by every later reader, instead of parsed
    /// again at every read (a biome file was parsed twice per generator).
    json_parse_cache,
    "JSON_PARSE_CACHE"
);
/// Named density functions and noises of a pack parsed on every core, only
/// `RUSTCRAFT_OPT_PARALLEL_DEFS_LOAD=1` (off by default: about a millisecond of work, which waking
/// the pool's threads costs more than it saves; see `docs/perf.md`).
#[inline]
pub fn parallel_defs_load() -> bool {
    #[cfg(test)]
    if let Some(v) = forced("PARALLEL_DEFS_LOAD") {
        return v;
    }
    static S: LazyLock<bool> =
        LazyLock::new(|| rc_data::env_str!("RUSTCRAFT_OPT_PARALLEL_DEFS_LOAD").is_some_and(|v| v.trim() == "1"));
    *S
}
switch_f!(
    /// Biome parameter list of a preset parsed once per world data, then copied into each generator.
    shared_climate_lists,
    "SHARED_CLIMATE_LISTS"
);
switch_f!(
    /// Processor lists built once per world data and shared by every decorator and structure
    /// generator, instead of twice per generator with structures.
    shared_proc_lists,
    "SHARED_PROC_LISTS"
);
switch_f!(
    /// Noise settings file parsed once per generator: the spawn targets are read from the copy kept
    /// by the settings instead of parsing the file a second time.
    noise_settings_read_once,
    "NOISE_SETTINGS_READ_ONCE"
);
switch_f!(
    /// Noise instance already created: found before its parameters are copied.
    noise_lookup_before_clone,
    "NOISE_LOOKUP_BEFORE_CLONE"
);
switch_f!(
    /// Spawn search on the climate noise: the points of a spiral evaluated on every core, the best
    /// one then picked in spiral order (same point as one at a time).
    parallel_spawn_search,
    "PARALLEL_SPAWN_SEARCH"
);
